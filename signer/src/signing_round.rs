use crate::key_share_store::StoredKeyShare;
use crate::round_delivery::{DeliveryBlame, InboxReceiver, InboxSender};
use crate::signer_state::{remove_round_inbox, SignerState};
use crate::threshold_protocol::{run_interactive_signing, EcdsaSignatureWithRecovery, RoundChannel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const MAXIMUM_FAILURE_DETAIL_CHARACTERS: usize = 300;

fn party_position_in_library_error(error_text: &str) -> Option<usize> {
    let after_marker = error_text.split("([(").nth(1)?;
    after_marker.split(',').next()?.trim().parse().ok()
}

// cggmp21 0.6.3 keeps its abort reason private: only the Debug text shows it, so this reads the pinned version's text.
fn attribute_failure(error_text: &str, blame: &DeliveryBlame, participants: &[String], own_signer_id: &str) -> Option<String> {
    if let Some(blamed_signer_id) = &blame.blamed_signer_id {
        return Some(blamed_signer_id.clone());
    }
    if let Some(position) = party_position_in_library_error(error_text) {
        return participants.get(position).cloned();
    }
    let others: Vec<&String> = participants.iter().filter(|signer| *signer != own_signer_id).collect();
    if others.len() == 1 { Some(others[0].clone()) } else { None }
}

fn describe_round_failure(error_text: &str, blame: &DeliveryBlame, participants: &[String], own_signer_id: &str) -> String {
    let attributed_signer_id = attribute_failure(error_text, blame, participants, own_signer_id).unwrap_or_else(|| "unknown".to_string());
    let detail: String = blame.detail.as_deref().unwrap_or(error_text).chars().take(MAXIMUM_FAILURE_DETAIL_CHARACTERS).collect();
    format!("round_failed peer={attributed_signer_id} detail={detail}")
}

pub struct SigningRound {
    pub session_id: String,
    pub participants: Vec<String>,
    pub stored_share: Arc<StoredKeyShare>,
    pub digest: [u8; 32],
    pub inbox_sender: InboxSender,
    pub inbox_receiver: InboxReceiver,
}

pub async fn run_signing_round(state: Arc<SignerState>, round: SigningRound) -> Result<EcdsaSignatureWithRecovery, String> {
    let own_position = round.participants.iter().position(|signer| *signer == state.config.signer_id).ok_or("not a participant")? as u16;
    let keygen_indices: Vec<u16> = round.participants.iter()
        .filter_map(|participant| round.stored_share.committee.iter().position(|signer| signer == participant))
        .map(|position| position as u16)
        .collect();
    let blame = Arc::new(Mutex::new(DeliveryBlame::default()));
    let channel = RoundChannel {
        directory: state.directory.clone(),
        session_id: round.session_id.clone(),
        participants: round.participants.clone(),
        inbox_sender: round.inbox_sender,
        inbox_receiver: round.inbox_receiver,
        blame: blame.clone(),
    };
    let round_deadline = Duration::from_secs(state.config.round_timeout_seconds);
    let runtime_handle = tokio::runtime::Handle::current();
    let stored_share = round.stored_share.clone();
    let digest = round.digest;
    // Each protocol step is CPU heavy with no yield point, so it runs off the server's worker threads.
    let protocol_outcome = tokio::task::spawn_blocking(move || {
        runtime_handle.block_on(async {
            tokio::time::timeout(
                round_deadline,
                run_interactive_signing(channel, own_position, &keygen_indices, &stored_share.key_share, &digest),
            )
            .await
        })
    })
    .await;
    remove_round_inbox(&state, &round.session_id);
    let delivery_blame = std::mem::take(&mut *blame.lock().expect("blame lock"));
    match protocol_outcome {
        Ok(Ok(Ok(signature))) => Ok(signature),
        Ok(Ok(Err(error_text))) => Err(describe_round_failure(&error_text, &delivery_blame, &round.participants, &state.config.signer_id)),
        Ok(Err(_)) => Err(describe_round_failure("round timed out", &delivery_blame, &round.participants, &state.config.signer_id)),
        Err(join_error) => Err(format!("round_failed peer=none detail=protocol task died: {join_error}")),
    }
}
