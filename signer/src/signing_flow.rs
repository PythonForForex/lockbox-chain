use crate::chain_view::{check_share_matches_chain, ensure_caught_up, read_chain_view, read_or_load_record, ChainView};
use crate::decision_recorder::{record_decision, DecisionEntry};
use crate::hyperliquid_action::{build_action_and_digest, BuiltAction};
use crate::key_share_store::StoredKeyShare;
use crate::nonce_record::{check_and_reserve, check_venue_nonce_window, persist_record, size_spent_today};
use crate::order_intent::{is_valid_identifier, parse_and_verify_intent, sha256_hex, OrderIntent, VerifiedIntent};
use crate::peer_transport::call_peer;
use crate::policy_rules::{evaluate_intent, ChainFreshness};
use crate::peer_signing_handlers::{participants_in_keygen_order, sweep_abandoned_sessions};
use crate::round_delivery::build_session_inbox;
use crate::signer_state::{lookup_key_share, now_unix_seconds, register_round_inbox, remove_round_inbox, SignerState};
use crate::signing_round::{run_signing_round, SigningRound};
use futures::stream::{FuturesUnordered, StreamExt};
use rand_core::{OsRng, RngCore};
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

const PROPOSAL_TIMEOUT_MILLISECONDS: u64 = 5000;
const BEGIN_TIMEOUT_MILLISECONDS: u64 = 3000;

pub fn record_refusal_of_raw(state: &SignerState, raw_intent: &Value, reason: &str) {
    let text_of = |field_name: &str| {
        raw_intent[field_name].as_str().filter(|text| is_valid_identifier(text)).unwrap_or("unparsed").to_string()
    };
    record_decision(
        &state.recorder,
        DecisionEntry {
            request_id: text_of("request_id"),
            key_ref: text_of("key_ref"),
            signer_id: state.config.signer_id.clone(),
            decision: "refused".to_string(),
            reason: reason.to_string(),
            intent_hash: sha256_hex(raw_intent.to_string().as_bytes()),
        },
    );
}

pub fn record_outcome(state: &SignerState, verified_intent: &VerifiedIntent, decision: &str, reason: &str) {
    record_decision(
        &state.recorder,
        DecisionEntry {
            request_id: verified_intent.intent.request_id.clone(),
            key_ref: verified_intent.intent.key_ref.clone(),
            signer_id: state.config.signer_id.clone(),
            decision: decision.to_string(),
            reason: reason.to_string(),
            intent_hash: verified_intent.intent_hash.clone(),
        },
    );
}

pub struct EvaluatedIntent {
    pub verified_intent: Arc<VerifiedIntent>,
    pub built_action: BuiltAction,
    pub stored_share: Arc<StoredKeyShare>,
    pub chain_view: ChainView,
}

pub async fn evaluate_without_reserving(state: &SignerState, raw_intent: Value) -> Result<EvaluatedIntent, String> {
    let verified_intent = Arc::new(parse_and_verify_intent(raw_intent)?);
    let intent = &verified_intent.intent;
    let stored_share = lookup_key_share(state, &intent.key_ref)?.ok_or("no_key_share_for_key_ref")?;
    ensure_caught_up(state, &stored_share).await?;
    let built_action = build_action_and_digest(intent)?;
    let chain_view = read_chain_view(state, &intent.key_ref, &intent.delegate_key).await?;
    check_share_matches_chain(&stored_share, &chain_view.key_reference)?;
    read_or_load_record(state, &intent.key_ref).await?;
    check_rules_and_nonce(state, &verified_intent.intent, &chain_view, false).await?;
    Ok(EvaluatedIntent { verified_intent: verified_intent.clone(), built_action, stored_share, chain_view })
}

// With persist true the record reaches the disk before this returns, and nothing leaves this signer for the round
// until then. The record lock is held across the check and the write, so two requests cannot both take one nonce.
pub async fn check_rules_and_nonce(state: &SignerState, intent: &OrderIntent, chain_view: &ChainView, persist: bool) -> Result<bool, String> {
    let mut nonce_records = state.nonce_records.lock().await;
    let record = nonce_records.get(&intent.key_ref).ok_or("nonce record missing")?;
    let now_seconds = now_unix_seconds();
    let allowed = evaluate_intent(
        intent,
        &chain_view.policy,
        &chain_view.freshness,
        &state.config.asset_index_by_name,
        size_spent_today(record, &intent.delegate_key, intent.asset.as_deref().unwrap_or_default(), now_seconds),
    )?;
    check_venue_nonce_window(intent.venue_nonce, now_seconds, state.config.venue_nonce_max_ahead_milliseconds)?;
    let reserved_record =
        check_and_reserve(record, intent.venue_nonce, &intent.delegate_key, allowed.new_risk_size.as_ref(), now_seconds)?;
    if persist {
        persist_record(&state.config.state_directory, &reserved_record)?;
        nonce_records.insert(intent.key_ref.clone(), reserved_record);
    }
    Ok(allowed.new_risk_size.is_some())
}

fn refusal_response(request_id: &str, reason: &str) -> Value {
    json!({"request_id": request_id, "signed": false, "reason": reason})
}

struct ProposalReply {
    peer_signer_id: String,
    outcome: Result<Value, String>,
}

async fn tell_peer(state: Arc<SignerState>, peer_signer_id: String, kind: &'static str, body: Value) {
    let timeout = Duration::from_millis(BEGIN_TIMEOUT_MILLISECONDS);
    if let Err(detail) = call_peer(&state.directory, &peer_signer_id, kind, &body, timeout).await {
        eprintln!("{kind} to {peer_signer_id} failed: {detail}");
    }
}

fn accepted_digest_matches(reply: &Value, own_digest_hex: &str) -> Result<bool, String> {
    if reply["accepted"].as_bool() != Some(true) {
        return Ok(false);
    }
    if reply["digest"].as_str() != Some(own_digest_hex) {
        return Err(format!("digest_mismatch_reported={}", reply["digest"]));
    }
    Ok(true)
}

// Sends the proposal to every committee member so each one's nonce record and exposure counter stay current, and
// proceeds as soon as enough have accepted. A slow or dead signer does not hold up the round.
// This signer reserves only after the quorum accepted: a peer catching up reads this signer's record, and if the nonce
// of this very request were already in it, the peer would refuse the request as a repeat.
pub async fn handle_sign_request(state: &Arc<SignerState>, raw_intent: Value) -> Value {
    sweep_abandoned_sessions(state);
    let reserved = match evaluate_without_reserving(state, raw_intent.clone()).await {
        Ok(evaluated) => evaluated,
        Err(reason) => {
            record_refusal_of_raw(state, &raw_intent, &reason);
            return refusal_response(raw_intent["request_id"].as_str().unwrap_or(""), &reason);
        }
    };
    let request_id = reserved.verified_intent.intent.request_id.clone();
    let digest_hex = hex::encode(reserved.built_action.digest);
    let mut random_bytes = [0u8; 32];
    OsRng.fill_bytes(&mut random_bytes);
    let session_id = sha256_hex(&[random_bytes.as_slice(), reserved.verified_intent.intent_hash.as_bytes()].concat());

    let (inbox_sender, inbox_receiver) = build_session_inbox();
    if let Err(reason) = register_round_inbox(state, &session_id, inbox_sender.clone()) {
        record_outcome(state, &reserved.verified_intent, "refused", &reason);
        return refusal_response(&request_id, &reason);
    }
    let own_signer_id = state.config.signer_id.clone();
    let threshold = reserved.stored_share.threshold as usize;
    let proposal_body = json!({
        "session_id": session_id,
        "intent": raw_intent,
        "height": state.highest_known_height.load(Ordering::SeqCst),
    });
    let mut pending_replies = FuturesUnordered::new();
    for peer_signer_id in reserved.stored_share.committee.iter().filter(|signer| **signer != own_signer_id).cloned() {
        let reply_state = state.clone();
        let reply_body = proposal_body.clone();
        pending_replies.push(async move {
            let timeout = Duration::from_millis(PROPOSAL_TIMEOUT_MILLISECONDS);
            let outcome = call_peer(&reply_state.directory, &peer_signer_id, "sign_proposal", &reply_body, timeout).await;
            ProposalReply { peer_signer_id, outcome }
        });
    }

    let mut accepted_peers: Vec<String> = Vec::new();
    let mut refusal_notes: Vec<String> = Vec::new();
    while accepted_peers.len() < threshold - 1 {
        let Some(reply) = pending_replies.next().await else {
            break;
        };
        match reply.outcome {
            Err(detail) => refusal_notes.push(detail),
            Ok(body) => {
                state.highest_known_height.fetch_max(body["height"].as_u64().unwrap_or(0), Ordering::SeqCst);
                match accepted_digest_matches(&body, &digest_hex) {
                    Ok(true) => accepted_peers.push(reply.peer_signer_id),
                    Ok(false) => refusal_notes.push(format!("{} refused: {}", reply.peer_signer_id, body["reason"].as_str().unwrap_or("no reason"))),
                    Err(detail) => refusal_notes.push(format!("{} peer={} {detail}", "action_mismatch", reply.peer_signer_id)),
                }
            }
        }
    }

    let quorum_reason = if accepted_peers.len() < threshold - 1 {
        Some(format!("quorum_unavailable {}", refusal_notes.join("; ")))
    } else {
        let refreshed_view = ChainView {
            freshness: ChainFreshness {
                highest_known_height: state.highest_known_height.load(Ordering::SeqCst),
                ..reserved.chain_view.freshness
            },
            policy: reserved.chain_view.policy.clone(),
            key_reference: reserved.chain_view.key_reference.clone(),
        };
        check_rules_and_nonce(state, &reserved.verified_intent.intent, &refreshed_view, true).await.err()
    };
    let mut chosen = accepted_peers.clone();
    chosen.push(own_signer_id.clone());
    let participants = participants_in_keygen_order(&reserved.stored_share.committee, &chosen);

    if let Some(reason) = quorum_reason {
        remove_round_inbox(state, &session_id);
        let abort_body = json!({"session_id": session_id, "reason": reason});
        for peer_signer_id in accepted_peers {
            tell_peer(state.clone(), peer_signer_id, "sign_abort", abort_body.clone()).await;
        }
        let late_state = state.clone();
        tokio::spawn(async move {
            while let Some(reply) = pending_replies.next().await {
                if matches!(reply.outcome, Ok(ref body) if body["accepted"].as_bool() == Some(true)) {
                    tell_peer(late_state.clone(), reply.peer_signer_id, "sign_abort", abort_body.clone()).await;
                }
            }
        });
        record_outcome(state, &reserved.verified_intent, "refused", &reason);
        return refusal_response(&request_id, &reason);
    }

    let begin_body = json!({"session_id": session_id, "participants": participants});
    let begin_timeout = Duration::from_millis(BEGIN_TIMEOUT_MILLISECONDS);
    for peer_signer_id in &accepted_peers {
        let begin_reply = call_peer(&state.directory, peer_signer_id, "sign_begin", &begin_body, begin_timeout).await;
        let started = matches!(&begin_reply, Ok(body) if body["started"].as_bool() == Some(true));
        if !started {
            let reason = format!("round_failed peer={peer_signer_id} detail=begin refused: {begin_reply:?}");
            remove_round_inbox(state, &session_id);
            record_outcome(state, &reserved.verified_intent, "refused", &reason);
            return refusal_response(&request_id, &reason);
        }
    }
    let late_state = state.clone();
    let late_begin_body = begin_body.clone();
    tokio::spawn(async move {
        while let Some(reply) = pending_replies.next().await {
            if matches!(reply.outcome, Ok(ref body) if body["accepted"].as_bool() == Some(true)) {
                tell_peer(late_state.clone(), reply.peer_signer_id, "sign_begin", late_begin_body.clone()).await;
            }
        }
    });

    let round = SigningRound {
        session_id: session_id.clone(),
        participants,
        stored_share: reserved.stored_share.clone(),
        digest: reserved.built_action.digest,
        inbox_sender,
        inbox_receiver,
    };
    match run_signing_round(state.clone(), round).await {
        Ok(signature) => {
            record_outcome(state, &reserved.verified_intent, "signed", "");
            json!({
                "request_id": request_id,
                "signed": true,
                "action": reserved.built_action.action,
                "digest": digest_hex,
                "r": signature.r,
                "s": signature.s,
                "v": signature.v,
            })
        }
        Err(reason) => {
            record_outcome(state, &reserved.verified_intent, "refused", &reason);
            refusal_response(&request_id, &reason)
        }
    }
}
