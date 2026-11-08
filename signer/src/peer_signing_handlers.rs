use crate::key_share_store::StoredKeyShare;
use crate::round_delivery::build_session_inbox;
use crate::signer_state::{register_round_inbox, remove_round_inbox, SignerState, SigningSession};
use crate::signing_flow::{check_rules_and_nonce, evaluate_without_reserving, record_outcome, record_refusal_of_raw};
use crate::signing_round::{run_signing_round, SigningRound};
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub fn sweep_abandoned_sessions(state: &SignerState) {
    let oldest_allowed = Duration::from_secs(state.config.round_timeout_seconds);
    let abandoned_sessions: Vec<SigningSession> = {
        let mut signing_sessions = state.signing_sessions.lock().expect("session lock");
        let abandoned_ids: Vec<String> = signing_sessions
            .iter()
            .filter(|(_, session)| session.created_at.elapsed() > oldest_allowed)
            .map(|(session_id, _)| session_id.clone())
            .collect();
        abandoned_ids.iter().filter_map(|session_id| {
            remove_round_inbox(state, session_id);
            signing_sessions.remove(session_id)
        }).collect()
    };
    for session in abandoned_sessions {
        record_outcome(state, &session.verified_intent, "refused", "round_never_started");
    }
}

pub async fn handle_sign_proposal(state: &Arc<SignerState>, sender: &str, body: &Value) -> Value {
    sweep_abandoned_sessions(state);
    state.highest_known_height.fetch_max(body["height"].as_u64().unwrap_or(0), Ordering::SeqCst);
    let height = state.highest_known_height.load(Ordering::SeqCst);
    let session_id = body["session_id"].as_str().unwrap_or_default().to_string();
    let raw_intent = body["intent"].clone();
    let evaluated = match evaluate_without_reserving(state, raw_intent.clone()).await {
        Ok(evaluated) => evaluated,
        Err(reason) => {
            record_refusal_of_raw(state, &raw_intent, &reason);
            return json!({"accepted": false, "reason": reason, "height": height});
        }
    };
    if let Err(reason) = check_rules_and_nonce(state, &evaluated.verified_intent.intent, &evaluated.chain_view, true).await {
        record_outcome(state, &evaluated.verified_intent, "refused", &reason);
        return json!({"accepted": false, "reason": reason, "height": height});
    }
    let (inbox_sender, inbox_receiver) = build_session_inbox();
    if let Err(reason) = register_round_inbox(state, &session_id, inbox_sender) {
        record_outcome(state, &evaluated.verified_intent, "refused", &reason);
        return json!({"accepted": false, "reason": reason, "height": height});
    }
    let digest = evaluated.built_action.digest;
    state.signing_sessions.lock().expect("session lock").insert(
        session_id,
        SigningSession {
            proposer: sender.to_string(),
            verified_intent: evaluated.verified_intent,
            digest,
            stored_share: evaluated.stored_share,
            inbox_receiver,
            created_at: Instant::now(),
        },
    );
    json!({"accepted": true, "digest": hex::encode(digest), "height": height})
}

pub fn participants_in_keygen_order(committee: &[String], chosen: &[String]) -> Vec<String> {
    committee.iter().filter(|signer| chosen.contains(signer)).cloned().collect()
}

fn check_participants(participants: &[String], stored_share: &StoredKeyShare, proposer: &str, own_signer_id: &str) -> Result<(), String> {
    let in_keygen_order = participants_in_keygen_order(&stored_share.committee, participants);
    if in_keygen_order != participants {
        return Err("participants_not_in_committee_order".to_string());
    }
    if participants.len() != stored_share.threshold as usize {
        return Err("participant_count_differs_from_threshold".to_string());
    }
    if !participants.iter().any(|signer| signer == proposer) || !participants.iter().any(|signer| signer == own_signer_id) {
        return Err("participants_omit_proposer_or_this_signer".to_string());
    }
    Ok(())
}

pub fn handle_sign_begin(state: &Arc<SignerState>, sender: &str, body: &Value) -> Value {
    let session_id = body["session_id"].as_str().unwrap_or_default();
    let session = {
        let mut signing_sessions = state.signing_sessions.lock().expect("session lock");
        match signing_sessions.get(session_id) {
            Some(session) if session.proposer == sender => signing_sessions.remove(session_id),
            _ => None,
        }
    };
    let Some(session) = session else {
        return json!({"started": false, "reason": "no_session_from_this_proposer"});
    };
    let participants: Vec<String> = serde_json::from_value(body["participants"].clone()).unwrap_or_default();
    if !participants.iter().any(|signer| *signer == state.config.signer_id) {
        remove_round_inbox(state, session_id);
        record_outcome(state, &session.verified_intent, "refused", "quorum_met_without_this_signer");
        return json!({"started": false, "reason": "not_in_round"});
    }
    if let Err(reason) = check_participants(&participants, &session.stored_share, sender, &state.config.signer_id) {
        remove_round_inbox(state, session_id);
        record_outcome(state, &session.verified_intent, "refused", &reason);
        return json!({"started": false, "reason": reason});
    }
    let inbox_sender = state.round_inboxes.lock().expect("round inbox lock").get(session_id).map(|inbox| inbox.sender.clone());
    let Some(inbox_sender) = inbox_sender else {
        return json!({"started": false, "reason": "session_inbox_gone"});
    };
    let round = SigningRound {
        session_id: session_id.to_string(),
        participants,
        stored_share: session.stored_share,
        digest: session.digest,
        inbox_sender,
        inbox_receiver: session.inbox_receiver,
    };
    let verified_intent = session.verified_intent;
    let task_state = state.clone();
    tokio::spawn(async move {
        match run_signing_round(task_state.clone(), round).await {
            Ok(_) => record_outcome(&task_state, &verified_intent, "signed", ""),
            Err(reason) => {
                eprintln!("request {} {reason}", verified_intent.intent.request_id);
                record_outcome(&task_state, &verified_intent, "refused", &reason);
            }
        }
    });
    json!({"started": true})
}

pub fn handle_sign_abort(state: &Arc<SignerState>, sender: &str, body: &Value) -> Value {
    let session_id = body["session_id"].as_str().unwrap_or_default();
    let session = {
        let mut signing_sessions = state.signing_sessions.lock().expect("session lock");
        match signing_sessions.get(session_id) {
            Some(session) if session.proposer == sender => signing_sessions.remove(session_id),
            _ => None,
        }
    };
    if let Some(session) = session {
        remove_round_inbox(state, session_id);
        record_outcome(state, &session.verified_intent, "refused", body["reason"].as_str().unwrap_or("aborted_by_proposer"));
    }
    json!({"aborted": true})
}
