use crate::decision_recorder::DecisionRecorder;
use crate::key_share_store::{load_committed_share, StoredKeyShare};
use crate::nonce_record::KeyNonceRecord;
use crate::order_intent::VerifiedIntent;
use crate::peer_transport::PeerDirectory;
use crate::round_delivery::{InboxEvent, InboxReceiver, InboxSender};
use crate::signer_config::SignerConfig;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex as AsyncMutex;

pub struct SigningSession {
    pub proposer: String,
    pub verified_intent: Arc<VerifiedIntent>,
    pub digest: [u8; 32],
    pub stored_share: Arc<StoredKeyShare>,
    pub inbox_receiver: InboxReceiver,
    pub created_at: Instant,
}

pub struct RoundInbox {
    pub sender: InboxSender,
    pub highest_sequence_by_sender: BTreeMap<String, u64>,
}

pub struct KeyGenerationCeremony {
    pub proposer: String,
    pub key_ref: String,
    pub committee: Vec<String>,
    pub threshold: u16,
    pub keygen_inbox_receiver: InboxReceiver,
    pub auxiliary_inbox_receiver: InboxReceiver,
}

pub struct SignerState {
    pub config: SignerConfig,
    pub directory: Arc<PeerDirectory>,
    pub recorder: Arc<DecisionRecorder>,
    pub node_http_client: reqwest::Client,
    pub key_shares: Mutex<BTreeMap<String, Arc<StoredKeyShare>>>,
    pub nonce_records: AsyncMutex<BTreeMap<String, KeyNonceRecord>>,
    pub caught_up_key_refs: Mutex<BTreeSet<String>>,
    pub signing_sessions: Mutex<BTreeMap<String, SigningSession>>,
    pub round_inboxes: Mutex<BTreeMap<String, RoundInbox>>,
    pub ceremonies: Mutex<BTreeMap<String, KeyGenerationCeremony>>,
    pub highest_known_height: AtomicU64,
}

pub fn now_unix_seconds() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|elapsed| elapsed.as_secs()).unwrap_or(0)
}

pub fn lookup_key_share(state: &SignerState, key_ref: &str) -> Result<Option<Arc<StoredKeyShare>>, String> {
    if let Some(stored_share) = state.key_shares.lock().expect("key share lock").get(key_ref) {
        return Ok(Some(stored_share.clone()));
    }
    let loaded_share = load_committed_share(&state.config.state_directory, key_ref)?;
    if let Some(stored_share) = &loaded_share {
        state.key_shares.lock().expect("key share lock").insert(key_ref.to_string(), stored_share.clone());
    }
    Ok(loaded_share)
}

pub fn register_round_inbox(state: &SignerState, session_id: &str, sender: InboxSender) -> Result<(), String> {
    let mut round_inboxes = state.round_inboxes.lock().expect("round inbox lock");
    if round_inboxes.contains_key(session_id) {
        return Err(format!("session {session_id} already exists"));
    }
    round_inboxes.insert(
        session_id.to_string(),
        RoundInbox { sender, highest_sequence_by_sender: BTreeMap::new() },
    );
    Ok(())
}

pub fn remove_round_inbox(state: &SignerState, session_id: &str) {
    state.round_inboxes.lock().expect("round inbox lock").remove(session_id);
}

pub fn deliver_round_message(
    state: &SignerState,
    session_id: &str,
    sender_signer_id: &str,
    sequence: u64,
    is_broadcast: bool,
    payload: String,
) -> Result<(), String> {
    let mut round_inboxes = state.round_inboxes.lock().expect("round inbox lock");
    let inbox = round_inboxes.get_mut(session_id).ok_or(format!("no session {session_id}"))?;
    let highest_sequence = inbox.highest_sequence_by_sender.entry(sender_signer_id.to_string()).or_insert(0);
    if sequence <= *highest_sequence {
        return Err(format!("repeated sequence {sequence} from {sender_signer_id}"));
    }
    *highest_sequence = sequence;
    let event = InboxEvent::ProtocolMessage {
        sender_signer_id: sender_signer_id.to_string(),
        is_broadcast,
        payload,
    };
    inbox.sender.try_send(event).map_err(|error| format!("session inbox refused: {error}"))
}
