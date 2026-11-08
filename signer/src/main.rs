mod chain_view;
mod cosmos_transaction;
mod decision_recorder;
mod http_api;
mod hyperliquid_action;
mod import_share_listener;
mod key_generation_flow;
mod key_import_flow;
mod key_share_store;
mod nonce_record;
mod order_intent;
mod peer_signing_handlers;
mod peer_transport;
mod policy_reader;
mod policy_rules;
mod round_delivery;
mod signer_config;
mod signer_state;
mod signing_flow;
mod signing_round;
mod threshold_protocol;

use decision_recorder::{build_decision_recorder, start_drain_worker};
use peer_transport::build_peer_directory;
use signer_config::load_signer_config;
use signer_state::SignerState;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};
use tokio::sync::Mutex as AsyncMutex;

#[tokio::main]
async fn main() -> Result<(), String> {
    let config_path = std::env::args().nth(1).ok_or("usage: lockbox-signer <config.json>")?;
    let config = load_signer_config(&config_path)?;
    std::fs::create_dir_all(&config.state_directory).map_err(|error| error.to_string())?;
    let recorder = Arc::new(build_decision_recorder(&config)?);
    let state = Arc::new(SignerState {
        directory: Arc::new(build_peer_directory(&config)?),
        recorder: recorder.clone(),
        node_http_client: policy_reader::build_node_http_client(),
        key_shares: Mutex::new(BTreeMap::new()),
        nonce_records: AsyncMutex::new(BTreeMap::new()),
        caught_up_key_refs: Mutex::new(BTreeSet::new()),
        signing_sessions: Mutex::new(BTreeMap::new()),
        round_inboxes: Mutex::new(BTreeMap::new()),
        ceremonies: Mutex::new(BTreeMap::new()),
        highest_known_height: AtomicU64::new(0),
        config: config.clone(),
    });
    start_drain_worker(&recorder);
    if let Some(import_share_listener_config) = config.import_share_listener.clone() {
        import_share_listener::start_import_share_listener(state.clone(), import_share_listener_config).await?;
    }
    let listener = tokio::net::TcpListener::bind(&config.listen_address).await.map_err(|error| error.to_string())?;
    eprintln!("lockbox-signer {} listening on {}", config.signer_id, config.listen_address);
    axum::serve(listener, http_api::build_router(state)).await.map_err(|error| error.to_string())
}
