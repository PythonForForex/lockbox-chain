use crate::nonce_record::{load_record, merge_peer_record, persist_record, KeyNonceRecord};
use crate::order_intent::sha256_hex;
use crate::peer_transport::call_peer;
use crate::policy_reader::{read_chain_status, read_key_ref, read_policy, ChainKeyRef, ChainPolicy};
use crate::policy_rules::ChainFreshness;
use crate::signer_state::{now_unix_seconds, SignerState};
use crate::key_share_store::StoredKeyShare;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;

const CATCH_UP_TIMEOUT_MILLISECONDS: u64 = 3000;

#[derive(Serialize, Deserialize)]
struct CachedChainItems {
    policy: ChainPolicy,
    key_reference: ChainKeyRef,
    local_height: u64,
    latest_block_time_seconds: u64,
}

pub struct ChainView {
    pub policy: ChainPolicy,
    pub key_reference: ChainKeyRef,
    pub freshness: ChainFreshness,
}

fn chain_cache_path(state: &SignerState, key_ref: &str, delegate_key: &str) -> PathBuf {
    let cache_name = sha256_hex(format!("{key_ref}/{delegate_key}").as_bytes());
    state.config.state_directory.join("chain_cache").join(format!("{cache_name}.json"))
}

fn build_freshness(state: &SignerState, node_reachable: bool, node_catching_up: bool, local_height: u64, latest_block_time_seconds: u64) -> ChainFreshness {
    ChainFreshness {
        now_seconds: now_unix_seconds(),
        node_reachable,
        node_catching_up,
        latest_block_time_seconds,
        local_height,
        highest_known_height: state.highest_known_height.load(Ordering::SeqCst),
        max_blocks_behind: state.config.max_blocks_behind,
        halt_timeout_seconds: state.config.halt_timeout_seconds,
    }
}

// A dead node is the stale-chain case, so the signer needs the last policy it saw in order to keep signing cancels
// and reduce-only orders. A live node that does not know the policy is a refusal and never falls back to the cache.
pub async fn read_chain_view(state: &SignerState, key_ref: &str, delegate_key: &str) -> Result<ChainView, String> {
    let http_client = &state.node_http_client;
    let chain_status = read_chain_status(http_client, &state.config.chain_rpc_url).await;
    let cache_path = chain_cache_path(state, key_ref, delegate_key);
    let Ok(status) = chain_status else {
        let raw_cache = fs::read(&cache_path).map_err(|_| "policy_unavailable_node_down_no_cache".to_string())?;
        let cached: CachedChainItems =
            serde_json::from_slice(&raw_cache).map_err(|_| "policy_unavailable_cache_corrupt".to_string())?;
        return Ok(ChainView {
            freshness: build_freshness(state, false, false, cached.local_height, cached.latest_block_time_seconds),
            policy: cached.policy,
            key_reference: cached.key_reference,
        });
    };
    state.highest_known_height.fetch_max(status.latest_block_height, Ordering::SeqCst);
    let (policy_result, key_reference_result) = tokio::join!(
        read_policy(http_client, &state.config.chain_rest_url, key_ref, delegate_key),
        read_key_ref(http_client, &state.config.chain_rest_url, key_ref)
    );
    let policy = policy_result.map_err(|error| format!("policy_unavailable: {error}"))?;
    let key_reference = key_reference_result.map_err(|error| format!("key_ref_unavailable: {error}"))?;
    let cached = CachedChainItems {
        policy: policy.clone(),
        key_reference: key_reference.clone(),
        local_height: status.latest_block_height,
        latest_block_time_seconds: status.latest_block_time_seconds,
    };
    if let Some(cache_directory) = cache_path.parent() {
        let _ = fs::create_dir_all(cache_directory);
    }
    let _ = fs::write(&cache_path, json!(cached).to_string());
    Ok(ChainView {
        freshness: build_freshness(state, true, status.catching_up, status.latest_block_height, status.latest_block_time_seconds),
        policy,
        key_reference,
    })
}

pub fn check_share_matches_chain(stored_share: &StoredKeyShare, key_reference: &ChainKeyRef) -> Result<(), String> {
    if key_reference.venue_address != stored_share.venue_address {
        return Err("key_ref_address_differs_from_share".to_string());
    }
    let mut chain_committee = key_reference.committee.clone();
    chain_committee.sort();
    if chain_committee != stored_share.committee || key_reference.threshold != stored_share.threshold as u32 {
        return Err("key_ref_committee_differs_from_share".to_string());
    }
    Ok(())
}

pub async fn read_or_load_record(state: &SignerState, key_ref: &str) -> Result<KeyNonceRecord, String> {
    let mut nonce_records = state.nonce_records.lock().await;
    if let Some(record) = nonce_records.get(key_ref) {
        return Ok(record.clone());
    }
    let record = load_record(&state.config.state_directory, key_ref)?;
    nonce_records.insert(key_ref.to_string(), record.clone());
    Ok(record)
}

async fn query_peer_record(state: &SignerState, peer_signer_id: &str, key_ref: &str) -> Result<Value, String> {
    let timeout = Duration::from_millis(CATCH_UP_TIMEOUT_MILLISECONDS);
    call_peer(&state.directory, peer_signer_id, "record_query", &json!({"key_ref": key_ref}), timeout).await
}

// The signer cannot tell a clean restart from a restore of an old backup, so every first use of a key after start is
// treated as a restore. Any two signing quorums overlap, so the highest record among the answers covers every signature
// the other signers know of. Runs on the request that needs it; there is no background timer.
pub async fn ensure_caught_up(state: &SignerState, stored_share: &StoredKeyShare) -> Result<(), String> {
    let key_ref = &stored_share.key_ref;
    if state.caught_up_key_refs.lock().expect("caught up lock").contains(key_ref) {
        return Ok(());
    }
    let required_answers = stored_share.threshold as usize - 1;
    let peer_signer_ids: Vec<&String> =
        stored_share.committee.iter().filter(|signer| **signer != state.config.signer_id).collect();
    let answers = futures::future::join_all(
        peer_signer_ids.iter().map(|peer_signer_id| query_peer_record(state, peer_signer_id, key_ref)),
    )
    .await;

    let mut own_record = read_or_load_record(state, key_ref).await?;
    let mut answered_count = 0usize;
    for (peer_signer_id, answer) in peer_signer_ids.iter().zip(answers) {
        let Ok(answer_body) = answer else {
            continue;
        };
        state.highest_known_height.fetch_max(answer_body["height"].as_u64().unwrap_or(0), Ordering::SeqCst);
        let Ok(peer_record) = serde_json::from_value::<KeyNonceRecord>(answer_body["record"].clone()) else {
            eprintln!("catch up: malformed record from {peer_signer_id}");
            continue;
        };
        match merge_peer_record(&own_record, &peer_record, now_unix_seconds(), state.config.venue_nonce_max_ahead_milliseconds) {
            Ok(merged_record) => {
                own_record = merged_record;
                answered_count += 1;
            }
            Err(detail) => eprintln!("catch up: ignored {peer_signer_id}: {detail}"),
        }
    }
    if answered_count < required_answers {
        return Err(format!("catching_up_waiting_for_quorum_{answered_count}_of_{required_answers}_peers_answered"));
    }
    persist_record(&state.config.state_directory, &own_record)?;
    state.nonce_records.lock().await.insert(key_ref.clone(), own_record);
    state.caught_up_key_refs.lock().expect("caught up lock").insert(key_ref.clone());
    Ok(())
}
