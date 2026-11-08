use crate::signer_config::SignerConfig;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::time::Duration;

const ENVELOPE_DOMAIN: &[u8] = b"lockbox-signer-peer-envelope-v1";
const PEER_CONNECT_TIMEOUT_MILLISECONDS: u64 = 1000;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PeerEnvelope {
    pub sender: String,
    pub recipient: String,
    pub kind: String,
    pub body: String,
    pub signature: String,
}

pub struct PeerEntry {
    pub url: String,
    pub identity_public_key: VerifyingKey,
}

pub struct PeerDirectory {
    pub own_signer_id: String,
    identity_key: SigningKey,
    pub peers: BTreeMap<String, PeerEntry>,
    http_client: reqwest::Client,
}

fn envelope_hash(sender: &str, recipient: &str, kind: &str, body: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(ENVELOPE_DOMAIN);
    for field in [sender, recipient, kind, body] {
        hasher.update((field.len() as u64).to_be_bytes());
        hasher.update(field.as_bytes());
    }
    hasher.finalize().into()
}

pub fn build_peer_directory(config: &SignerConfig) -> Result<PeerDirectory, String> {
    let raw_key = std::fs::read_to_string(&config.identity_private_key_file)
        .map_err(|error| format!("read identity key: {error}"))?;
    let key_bytes: [u8; 32] = hex::decode(raw_key.trim())
        .map_err(|_| "identity key is not hex")?
        .try_into()
        .map_err(|_| "identity key is not 32 bytes")?;
    let mut peers = BTreeMap::new();
    for peer in &config.peers {
        let public_key_bytes: [u8; 32] = hex::decode(&peer.identity_public_key)
            .map_err(|_| "peer identity key is not hex")?
            .try_into()
            .map_err(|_| "peer identity key is not 32 bytes")?;
        let identity_public_key =
            VerifyingKey::from_bytes(&public_key_bytes).map_err(|_| "peer identity key is invalid")?;
        peers.insert(peer.signer_id.clone(), PeerEntry { url: peer.url.clone(), identity_public_key });
    }
    let http_client = reqwest::Client::builder()
        .connect_timeout(Duration::from_millis(PEER_CONNECT_TIMEOUT_MILLISECONDS))
        .build()
        .map_err(|error| error.to_string())?;
    Ok(PeerDirectory {
        own_signer_id: config.signer_id.clone(),
        identity_key: SigningKey::from_bytes(&key_bytes),
        peers,
        http_client,
    })
}

pub fn seal_envelope(directory: &PeerDirectory, recipient: &str, kind: &str, body: &Value) -> PeerEnvelope {
    let body_text = body.to_string();
    let hash = envelope_hash(&directory.own_signer_id, recipient, kind, &body_text);
    PeerEnvelope {
        sender: directory.own_signer_id.clone(),
        recipient: recipient.to_string(),
        kind: kind.to_string(),
        signature: hex::encode(directory.identity_key.sign(&hash).to_bytes()),
        body: body_text,
    }
}

pub fn open_envelope(directory: &PeerDirectory, envelope: &PeerEnvelope) -> Result<Value, String> {
    if envelope.recipient != directory.own_signer_id {
        return Err(format!("envelope addressed to {}", envelope.recipient));
    }
    let peer = directory.peers.get(&envelope.sender).ok_or(format!("unknown sender {}", envelope.sender))?;
    let signature_bytes: [u8; 64] = hex::decode(&envelope.signature)
        .map_err(|_| "signature is not hex")?
        .try_into()
        .map_err(|_| "signature is not 64 bytes")?;
    let hash = envelope_hash(&envelope.sender, &envelope.recipient, &envelope.kind, &envelope.body);
    peer.identity_public_key
        .verify_strict(&hash, &Signature::from_bytes(&signature_bytes))
        .map_err(|_| format!("signature does not verify for claimed sender {}", envelope.sender))?;
    serde_json::from_str(&envelope.body).map_err(|_| format!("body from {} is not JSON", envelope.sender))
}

pub async fn call_peer(
    directory: &PeerDirectory,
    recipient: &str,
    kind: &str,
    body: &Value,
    timeout: Duration,
) -> Result<Value, String> {
    let peer = directory.peers.get(recipient).ok_or(format!("no peer {recipient}"))?;
    let envelope = seal_envelope(directory, recipient, kind, body);
    let response = directory
        .http_client
        .post(format!("{}/peer", peer.url))
        .timeout(timeout)
        .json(&envelope)
        .send()
        .await
        .map_err(|error| format!("{recipient} unreachable: {error}"))?;
    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(format!("{recipient} answered {status}: {text}"));
    }
    let reply_envelope: PeerEnvelope =
        response.json().await.map_err(|error| format!("{recipient} reply malformed: {error}"))?;
    if reply_envelope.sender != recipient {
        return Err(format!("{recipient} reply signed as {}", reply_envelope.sender));
    }
    open_envelope(directory, &reply_envelope)
}
