use crate::key_share_store::{commit_pending_share, committed_share_exists, load_committed_share, write_pending_share};
use crate::order_intent::sha256_hex;
use crate::peer_transport::call_peer;
use crate::round_delivery::{build_session_inbox, DeliveryBlame, InboxEvent};
use crate::signer_state::{register_round_inbox, remove_round_inbox, KeyGenerationCeremony, SignerState};
use crate::threshold_protocol::{run_distributed_key_generation, venue_address_of_share, RoundChannel};
use futures::future::join_all;
use rand_core::{OsRng, RngCore};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const MINIMUM_THRESHOLD: u16 = 2;
const PREPARE_TIMEOUT_MILLISECONDS: u64 = 5000;

fn keygen_session_id(ceremony_id: &str) -> String {
    format!("{ceremony_id}:keygen")
}

fn auxiliary_session_id(ceremony_id: &str) -> String {
    format!("{ceremony_id}:auxiliary")
}

pub fn check_ceremony_request(state: &SignerState, key_ref: &str, sorted_committee: &[String], threshold: u16) -> Result<(), String> {
    if key_ref.is_empty() {
        return Err("key_ref_empty".to_string());
    }
    if threshold < MINIMUM_THRESHOLD || threshold as usize > sorted_committee.len() {
        return Err("threshold_out_of_range".to_string());
    }
    if sorted_committee.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err("committee_has_duplicates".to_string());
    }
    if !sorted_committee.contains(&state.config.signer_id) {
        return Err("this_signer_not_in_committee".to_string());
    }
    for signer_id in sorted_committee {
        if *signer_id != state.config.signer_id && !state.directory.peers.contains_key(signer_id) {
            return Err(format!("unknown_committee_member_{signer_id}"));
        }
    }
    if committed_share_exists(&state.config.state_directory, key_ref) {
        return Err("key_ref_already_has_a_share".to_string());
    }
    Ok(())
}

fn parse_committee(body: &Value) -> Result<Vec<String>, String> {
    let mut committee: Vec<String> = serde_json::from_value(body["committee"].clone()).map_err(|_| "bad_committee")?;
    committee.sort();
    Ok(committee)
}

fn prepare_ceremony(state: &SignerState, proposer: &str, ceremony_id: &str, key_ref: &str, committee: Vec<String>, threshold: u16) -> Result<(), String> {
    check_ceremony_request(state, key_ref, &committee, threshold)?;
    let (keygen_sender, keygen_receiver) = build_session_inbox();
    let (auxiliary_sender, auxiliary_receiver) = build_session_inbox();
    register_round_inbox(state, &keygen_session_id(ceremony_id), keygen_sender)?;
    register_round_inbox(state, &auxiliary_session_id(ceremony_id), auxiliary_sender)?;
    state.ceremonies.lock().expect("ceremony lock").insert(
        ceremony_id.to_string(),
        KeyGenerationCeremony {
            proposer: proposer.to_string(),
            key_ref: key_ref.to_string(),
            committee,
            threshold,
            keygen_inbox_receiver: keygen_receiver,
            auxiliary_inbox_receiver: auxiliary_receiver,
        },
    );
    Ok(())
}

fn describe_ceremony_failure(error_text: &str, blame: &DeliveryBlame) -> String {
    match &blame.blamed_signer_id {
        Some(blamed_signer_id) => format!("dkg_failed peer={blamed_signer_id} detail={}", blame.detail.as_deref().unwrap_or(error_text)),
        None => format!("dkg_failed detail={error_text}"),
    }
}

async fn run_ceremony(state: Arc<SignerState>, ceremony_id: String) -> Result<String, String> {
    let ceremony = state.ceremonies.lock().expect("ceremony lock").remove(&ceremony_id).ok_or("no_such_ceremony")?;
    let own_index = ceremony.committee.iter().position(|signer| *signer == state.config.signer_id).ok_or("not_in_committee")? as u16;
    let committee_size = ceremony.committee.len() as u16;
    let keygen_session = keygen_session_id(&ceremony_id);
    let auxiliary_session = auxiliary_session_id(&ceremony_id);
    let sender_of = |session_id: &str| -> Result<_, String> {
        state.round_inboxes.lock().expect("round inbox lock").get(session_id).map(|inbox| inbox.sender.clone()).ok_or("ceremony inbox gone".to_string())
    };
    let keygen_blame = Arc::new(Mutex::new(DeliveryBlame::default()));
    let auxiliary_blame = Arc::new(Mutex::new(DeliveryBlame::default()));
    let keygen_channel = RoundChannel {
        directory: state.directory.clone(),
        session_id: keygen_session.clone(),
        participants: ceremony.committee.clone(),
        inbox_sender: sender_of(&keygen_session)?,
        inbox_receiver: ceremony.keygen_inbox_receiver,
        blame: keygen_blame.clone(),
    };
    let auxiliary_channel = RoundChannel {
        directory: state.directory.clone(),
        session_id: auxiliary_session.clone(),
        participants: ceremony.committee.clone(),
        inbox_sender: sender_of(&auxiliary_session)?,
        inbox_receiver: ceremony.auxiliary_inbox_receiver,
        blame: auxiliary_blame.clone(),
    };
    let ceremony_deadline = Duration::from_secs(state.config.dkg_timeout_seconds);
    let threshold = ceremony.threshold;
    let runtime_handle = tokio::runtime::Handle::current();
    // Prime generation and proof steps are CPU heavy with no yield point, so they run off the server's worker threads.
    let protocol_outcome = tokio::task::spawn_blocking(move || {
        runtime_handle.block_on(async {
            tokio::time::timeout(
                ceremony_deadline,
                run_distributed_key_generation(keygen_channel, auxiliary_channel, own_index, committee_size, threshold),
            )
            .await
        })
    })
    .await;
    remove_round_inbox(&state, &keygen_session);
    remove_round_inbox(&state, &auxiliary_session);
    let key_share = match protocol_outcome {
        Ok(Ok(Ok(key_share))) => key_share,
        Ok(Ok(Err(error_text))) => {
            let keygen_delivery_blame = std::mem::take(&mut *keygen_blame.lock().expect("blame lock"));
            let auxiliary_delivery_blame = std::mem::take(&mut *auxiliary_blame.lock().expect("blame lock"));
            let blame = if keygen_delivery_blame.detail.is_some() { keygen_delivery_blame } else { auxiliary_delivery_blame };
            return Err(describe_ceremony_failure(&error_text, &blame));
        }
        Ok(Err(_)) => return Err("dkg_failed detail=ceremony timed out".to_string()),
        Err(join_error) => return Err(format!("dkg_failed detail=protocol task died: {join_error}")),
    };
    write_pending_share(&state.config.state_directory, &ceremony.key_ref, &ceremony.committee, threshold, &key_share)?;
    Ok(venue_address_of_share(&key_share))
}

pub fn commit_ceremony(state: &SignerState, key_ref: &str) -> Result<(), String> {
    commit_pending_share(&state.config.state_directory, key_ref)?;
    let stored_share = load_committed_share(&state.config.state_directory, key_ref)?.ok_or("share vanished after commit")?;
    state.key_shares.lock().expect("key share lock").insert(key_ref.to_string(), stored_share);
    state.caught_up_key_refs.lock().expect("caught up lock").insert(key_ref.to_string());
    Ok(())
}

pub fn handle_dkg_prepare(state: &SignerState, sender: &str, body: &Value) -> Value {
    let outcome = parse_committee(body).and_then(|committee| {
        prepare_ceremony(
            state,
            sender,
            body["ceremony_id"].as_str().unwrap_or_default(),
            body["key_ref"].as_str().unwrap_or_default(),
            committee,
            body["threshold"].as_u64().unwrap_or(0) as u16,
        )
    });
    match outcome {
        Ok(()) => json!({"prepared": true}),
        Err(reason) => json!({"prepared": false, "reason": reason}),
    }
}

pub async fn handle_dkg_start(state: &Arc<SignerState>, sender: &str, body: &Value) -> Value {
    let ceremony_id = body["ceremony_id"].as_str().unwrap_or_default().to_string();
    let proposer_matches = state.ceremonies.lock().expect("ceremony lock").get(&ceremony_id).map(|ceremony| ceremony.proposer == sender) == Some(true);
    if !proposer_matches {
        return json!({"reason": "no_ceremony_from_this_proposer"});
    }
    match run_ceremony(state.clone(), ceremony_id).await {
        Ok(venue_address) => json!({"venue_address": venue_address}),
        Err(reason) => json!({"reason": reason}),
    }
}

pub fn handle_dkg_commit(state: &SignerState, body: &Value) -> Value {
    match commit_ceremony(state, body["key_ref"].as_str().unwrap_or_default()) {
        Ok(()) => json!({"committed": true}),
        Err(reason) => json!({"committed": false, "reason": reason}),
    }
}

// Wakes a protocol blocked on a dead peer's message. Without this the ceremony would sit until the DKG deadline.
fn fail_local_ceremony(state: &SignerState, ceremony_id: &str, blamed_signer_id: Option<&str>, detail: &str) {
    let round_inboxes = state.round_inboxes.lock().expect("round inbox lock");
    for session_id in [keygen_session_id(ceremony_id), auxiliary_session_id(ceremony_id)] {
        if let Some(inbox) = round_inboxes.get(&session_id) {
            let failure = InboxEvent::DeliveryFailure {
                blamed_signer_id: blamed_signer_id.map(str::to_string),
                detail: detail.to_string(),
            };
            let _ = inbox.sender.try_send(failure);
        }
    }
}

pub fn handle_dkg_abort(state: &SignerState, sender: &str, body: &Value) -> Value {
    fail_local_ceremony(state, body["ceremony_id"].as_str().unwrap_or_default(), Some(sender), body["reason"].as_str().unwrap_or("aborted"));
    json!({"aborted": true})
}

async fn start_peer_ceremony(state: &Arc<SignerState>, peer_signer_id: &str, ceremony_id: &str, timeout: Duration) -> Result<String, String> {
    let reply = call_peer(&state.directory, peer_signer_id, "dkg_start", &json!({"ceremony_id": ceremony_id}), timeout).await;
    let outcome = reply.and_then(|body| body["venue_address"].as_str().map(str::to_string).ok_or(format!("{peer_signer_id}: {}", body["reason"])));
    if let Err(detail) = &outcome {
        fail_local_ceremony(state, ceremony_id, Some(peer_signer_id), detail);
        let abort_body = json!({"ceremony_id": ceremony_id, "reason": detail});
        let notify_timeout = Duration::from_millis(PREPARE_TIMEOUT_MILLISECONDS);
        for other_signer_id in state.directory.peers.keys().filter(|signer| *signer != peer_signer_id) {
            let _ = call_peer(&state.directory, other_signer_id, "dkg_abort", &abort_body, notify_timeout).await;
        }
    }
    outcome
}

pub async fn handle_dkg_request(state: &Arc<SignerState>, body: &Value) -> Result<Value, String> {
    let key_ref = body["key_ref"].as_str().unwrap_or_default().to_string();
    let committee = parse_committee(body)?;
    let threshold = body["threshold"].as_u64().unwrap_or(0) as u16;
    let mut random_bytes = [0u8; 32];
    OsRng.fill_bytes(&mut random_bytes);
    let ceremony_id = sha256_hex(&random_bytes);
    let own_signer_id = state.config.signer_id.clone();
    prepare_ceremony(state, &own_signer_id, &ceremony_id, &key_ref, committee.clone(), threshold)?;

    let peers: Vec<String> = committee.iter().filter(|signer| **signer != own_signer_id).cloned().collect();
    let prepare_body = json!({"ceremony_id": ceremony_id, "key_ref": key_ref, "committee": committee, "threshold": threshold});
    let prepare_timeout = Duration::from_millis(PREPARE_TIMEOUT_MILLISECONDS);
    for peer_signer_id in &peers {
        let reply = call_peer(&state.directory, peer_signer_id, "dkg_prepare", &prepare_body, prepare_timeout).await?;
        if reply["prepared"].as_bool() != Some(true) {
            return Err(format!("{peer_signer_id} refused: {}", reply["reason"]));
        }
    }
    let ceremony_timeout = Duration::from_secs(state.config.dkg_timeout_seconds);
    let peer_outcomes = join_all(peers.iter().map(|peer_signer_id| start_peer_ceremony(state, peer_signer_id, &ceremony_id, ceremony_timeout)));
    let (own_outcome, peer_outcomes) = tokio::join!(run_ceremony(state.clone(), ceremony_id.clone()), peer_outcomes);
    let venue_address = own_outcome?;
    for outcome in peer_outcomes {
        let peer_address = outcome?;
        if peer_address != venue_address {
            return Err(format!("dkg_failed committee disagrees on the address: {peer_address} and {venue_address}"));
        }
    }
    for peer_signer_id in &peers {
        let reply = call_peer(&state.directory, peer_signer_id, "dkg_commit", &json!({"key_ref": key_ref}), prepare_timeout).await?;
        if reply["committed"].as_bool() != Some(true) {
            return Err(format!("{peer_signer_id} did not commit: {}", reply["reason"]));
        }
    }
    commit_ceremony(state, &key_ref)?;
    Ok(json!({"venue_address": venue_address}))
}
