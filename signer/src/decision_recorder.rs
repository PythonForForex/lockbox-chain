use crate::cosmos_transaction::{build_record_decision_transaction, load_chain_signing_identity, transaction_hash_hex, ChainSigningIdentity};
use crate::nonce_record::write_file_durably;
use crate::order_intent::sha256_hex;
use crate::signer_config::SignerConfig;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Notify;

const MAXIMUM_REASON_BYTES: usize = 512;
const CHAIN_REQUEST_TIMEOUT_MILLISECONDS: u64 = 3000;
const MAXIMUM_IMMEDIATE_PASSES: usize = 5;
const CODE_ACCEPTED: u64 = 0;
const CODE_ALREADY_IN_MEMPOOL: u64 = 19;
const CODE_WRONG_SEQUENCE: u64 = 32;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DecisionEntry {
    pub request_id: String,
    pub key_ref: String,
    pub signer_id: String,
    pub decision: String,
    pub reason: String,
    pub intent_hash: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SentTransaction {
    transaction_hex: String,
    sequence: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct OutboxItem {
    entry: DecisionEntry,
    sent: Option<SentTransaction>,
}

pub struct DecisionRecorder {
    outbox_directory: PathBuf,
    rejected_directory: PathBuf,
    chain_rest_url: String,
    identity: ChainSigningIdentity,
    http_client: reqwest::Client,
    drain_wake: Notify,
}

enum BroadcastOutcome {
    InMempool,
    WrongSequence(Option<u64>),
    Rejected(String),
}

enum PassOutcome {
    Settled,
    RetryFrom(u64),
}

pub fn build_decision_recorder(config: &SignerConfig) -> Result<DecisionRecorder, String> {
    let outbox_directory = config.state_directory.join("decision_outbox");
    let rejected_directory = config.state_directory.join("decision_rejected");
    fs::create_dir_all(&outbox_directory).map_err(|error| error.to_string())?;
    fs::create_dir_all(&rejected_directory).map_err(|error| error.to_string())?;
    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_millis(CHAIN_REQUEST_TIMEOUT_MILLISECONDS))
        .build()
        .map_err(|error| error.to_string())?;
    Ok(DecisionRecorder {
        outbox_directory,
        rejected_directory,
        chain_rest_url: config.chain_rest_url.clone(),
        identity: load_chain_signing_identity(config)?,
        http_client,
        drain_wake: Notify::new(),
    })
}

fn shorten_reason(reason: &str) -> String {
    let mut end = reason.len().min(MAXIMUM_REASON_BYTES);
    while !reason.is_char_boundary(end) {
        end -= 1;
    }
    reason[..end].to_string()
}

fn outbox_entry_name(entry: &DecisionEntry) -> String {
    let nanoseconds = SystemTime::now().duration_since(UNIX_EPOCH).map(|elapsed| elapsed.as_nanos()).unwrap_or(0);
    let identity = format!("{}{}{}", entry.request_id, entry.signer_id, entry.reason);
    format!("{nanoseconds:032}-{}.json", &sha256_hex(identity.as_bytes())[..16])
}

fn read_unsigned_text(value: &Value) -> Option<u64> {
    value.as_str().and_then(|text| text.parse().ok()).or(value.as_u64())
}

async fn read_account_position(recorder: &DecisionRecorder) -> Result<(u64, u64), String> {
    let url = format!("{}/cosmos/auth/v1beta1/accounts/{}", recorder.chain_rest_url, recorder.identity.account_address);
    let response = recorder.http_client.get(url).send().await.map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!("account query answered {}", response.status()));
    }
    let document: Value = response.json().await.map_err(|error| error.to_string())?;
    let account = &document["account"];
    let account_number = read_unsigned_text(&account["account_number"]).unwrap_or(0);
    let sequence = read_unsigned_text(&account["sequence"]).unwrap_or(0);
    Ok((account_number, sequence))
}

// The node's mempool can be ahead of the committed account, for example with transactions from before a restore from
// backup, whose outbox files are gone. Only the error text says where it is: "account sequence mismatch, expected 60, got 58".
fn expected_sequence_in(raw_log: &str) -> Option<u64> {
    raw_log.split("expected ").nth(1)?.split(',').next()?.trim().parse().ok()
}

async fn broadcast_transaction(recorder: &DecisionRecorder, transaction_bytes: &[u8]) -> Result<BroadcastOutcome, String> {
    let url = format!("{}/cosmos/tx/v1beta1/txs", recorder.chain_rest_url);
    let body = json!({
        "tx_bytes": base64::engine::general_purpose::STANDARD.encode(transaction_bytes),
        "mode": "BROADCAST_MODE_SYNC",
    });
    let response = recorder.http_client.post(url).json(&body).send().await.map_err(|error| error.to_string())?;
    let status = response.status();
    let document: Value = response.json().await.map_err(|error| format!("broadcast answered {status}: {error}"))?;
    let Some(code) = document["tx_response"]["code"].as_u64() else {
        return Err(format!("broadcast answered {status}: {document}"));
    };
    Ok(match code {
        CODE_ACCEPTED | CODE_ALREADY_IN_MEMPOOL => BroadcastOutcome::InMempool,
        CODE_WRONG_SEQUENCE => {
            let raw_log = document["tx_response"]["raw_log"].as_str().unwrap_or_default();
            eprintln!("decision broadcast wrong sequence: {raw_log}");
            BroadcastOutcome::WrongSequence(expected_sequence_in(raw_log))
        }
        _ => BroadcastOutcome::Rejected(format!("code {code}: {}", document["tx_response"]["raw_log"])),
    })
}

async fn delivered_failure(recorder: &DecisionRecorder, transaction_bytes: &[u8]) -> Option<String> {
    let url = format!("{}/cosmos/tx/v1beta1/txs/{}", recorder.chain_rest_url, transaction_hash_hex(transaction_bytes));
    let response = recorder.http_client.get(url).send().await.ok()?;
    let document: Value = response.json().await.ok()?;
    let code = document["tx_response"]["code"].as_u64()?;
    (code != CODE_ACCEPTED).then(|| format!("code {code}: {}", document["tx_response"]["raw_log"]))
}

fn move_to_rejected(recorder: &DecisionRecorder, path: &Path, why: &str) {
    eprintln!("decision rejected by the chain, kept in decision_rejected: {why}");
    if let Some(file_name) = path.file_name() {
        let _ = fs::rename(path, recorder.rejected_directory.join(file_name));
    }
}

fn sorted_outbox_paths(recorder: &DecisionRecorder) -> Vec<PathBuf> {
    let Ok(directory_entries) = fs::read_dir(&recorder.outbox_directory) else {
        return Vec::new();
    };
    let mut entry_paths: Vec<PathBuf> = directory_entries
        .filter_map(|item| item.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().and_then(|extension| extension.to_str()) == Some("json"))
        .collect();
    entry_paths.sort();
    entry_paths
}

// One entry is one transaction. A sent entry below the chain's sequence has landed: this key signs nothing else and a
// sequence is only reused after an entry is cleared, so only this transaction can have consumed it. An entry stays on
// disk until that is seen. Sending the same bytes again is harmless, which covers a mempool lost in a node restart.
async fn drain_one_pass(recorder: &DecisionRecorder, sequence_floor: u64) -> Result<PassOutcome, String> {
    let (account_number, chain_sequence) = read_account_position(recorder).await?;
    let mut next_sequence = chain_sequence.max(sequence_floor);
    for entry_path in sorted_outbox_paths(recorder) {
        let Some(mut item) = fs::read(&entry_path).ok().and_then(|raw| serde_json::from_slice::<OutboxItem>(&raw).ok()) else {
            continue;
        };
        let mut resent_bytes = None;
        if let Some(sent) = &item.sent {
            let sent_bytes = hex::decode(&sent.transaction_hex).map_err(|error| error.to_string())?;
            if sent.sequence < chain_sequence {
                match delivered_failure(recorder, &sent_bytes).await {
                    Some(why) => move_to_rejected(recorder, &entry_path, &why),
                    None => {
                        let _ = fs::remove_file(&entry_path);
                    }
                }
                continue;
            }
            if sent.sequence == next_sequence {
                resent_bytes = Some(sent_bytes);
            }
        }
        let is_resend = resent_bytes.is_some();
        let transaction_bytes = match resent_bytes {
            Some(transaction_bytes) => transaction_bytes,
            None => {
                let built = build_record_decision_transaction(&recorder.identity, &item.entry, account_number, next_sequence);
                item.sent = Some(SentTransaction { transaction_hex: hex::encode(&built), sequence: next_sequence });
                write_file_durably(&entry_path, json!(item).to_string().as_bytes())?;
                built
            }
        };
        match broadcast_transaction(recorder, &transaction_bytes).await? {
            BroadcastOutcome::InMempool => next_sequence += 1,
            BroadcastOutcome::WrongSequence(expected_sequence) => {
                // Bytes built just now were never taken, so they must not count as sent: a later pass would read their
                // sequence as used and drop the entry. A resend may have landed already and stays as it is.
                if !is_resend {
                    item.sent = None;
                    write_file_durably(&entry_path, json!(item).to_string().as_bytes())?;
                }
                return Ok(PassOutcome::RetryFrom(expected_sequence.unwrap_or(0)));
            }
            BroadcastOutcome::Rejected(why) => move_to_rejected(recorder, &entry_path, &why),
        }
    }
    Ok(PassOutcome::Settled)
}

async fn drain_until_settled(recorder: &DecisionRecorder) {
    let mut sequence_floor = 0;
    for _ in 0..MAXIMUM_IMMEDIATE_PASSES {
        match drain_one_pass(recorder, sequence_floor).await {
            Ok(PassOutcome::RetryFrom(expected_sequence)) => sequence_floor = expected_sequence,
            Ok(PassOutcome::Settled) => return,
            Err(detail) => {
                eprintln!("decision drain paused: {detail}");
                return;
            }
        }
    }
}

// Each decision and each start wakes the worker. There is no retry timer: whatever a pass could not deliver waits on
// disk for the next event. The order path only writes the file and wakes this task.
pub fn start_drain_worker(recorder: &Arc<DecisionRecorder>) {
    let worker_recorder = recorder.clone();
    tokio::spawn(async move {
        loop {
            drain_until_settled(&worker_recorder).await;
            worker_recorder.drain_wake.notified().await;
        }
    });
}

pub fn record_decision(recorder: &Arc<DecisionRecorder>, mut entry: DecisionEntry) {
    entry.reason = shorten_reason(&entry.reason);
    let target_path = recorder.outbox_directory.join(outbox_entry_name(&entry));
    let item = OutboxItem { entry, sent: None };
    if let Err(error) = write_file_durably(&target_path, json!(item).to_string().as_bytes()) {
        eprintln!("decision outbox write failed for {}: {error}", item.entry.request_id);
    }
    recorder.drain_wake.notify_one();
}
