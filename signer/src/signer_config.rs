use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::PathBuf;

const DEFAULT_MAX_BLOCKS_BEHIND: u64 = 10;
const DEFAULT_HALT_TIMEOUT_SECONDS: u64 = 180;
const DEFAULT_VENUE_NONCE_MAX_AHEAD_MILLISECONDS: u64 = 300_000;
const DEFAULT_ROUND_TIMEOUT_SECONDS: u64 = 30;
const DEFAULT_DKG_TIMEOUT_SECONDS: u64 = 900;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerConfig {
    pub signer_id: String,
    pub url: String,
    pub identity_public_key: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportShareListenerConfig {
    pub listen_address: String,
    pub certificate_file: PathBuf,
    pub private_key_file: PathBuf,
    pub pinned_client_certificate_file: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignerConfig {
    pub signer_id: String,
    pub listen_address: String,
    pub state_directory: PathBuf,
    pub identity_private_key_file: PathBuf,
    pub peers: Vec<PeerConfig>,
    pub chain_rest_url: String,
    pub chain_rpc_url: String,
    pub chain_id: String,
    pub chain_key_file: PathBuf,
    pub chain_account_address: String,
    #[serde(default = "default_max_blocks_behind")]
    pub max_blocks_behind: u64,
    #[serde(default = "default_halt_timeout_seconds")]
    pub halt_timeout_seconds: u64,
    #[serde(default = "default_venue_nonce_max_ahead_milliseconds")]
    pub venue_nonce_max_ahead_milliseconds: u64,
    #[serde(default = "default_round_timeout_seconds")]
    pub round_timeout_seconds: u64,
    #[serde(default = "default_dkg_timeout_seconds")]
    pub dkg_timeout_seconds: u64,
    #[serde(default)]
    pub asset_index_by_name: BTreeMap<String, u32>,
    #[serde(default)]
    pub import_share_listener: Option<ImportShareListenerConfig>,
}

fn default_max_blocks_behind() -> u64 {
    DEFAULT_MAX_BLOCKS_BEHIND
}

fn default_halt_timeout_seconds() -> u64 {
    DEFAULT_HALT_TIMEOUT_SECONDS
}

fn default_venue_nonce_max_ahead_milliseconds() -> u64 {
    DEFAULT_VENUE_NONCE_MAX_AHEAD_MILLISECONDS
}

fn default_round_timeout_seconds() -> u64 {
    DEFAULT_ROUND_TIMEOUT_SECONDS
}

fn default_dkg_timeout_seconds() -> u64 {
    DEFAULT_DKG_TIMEOUT_SECONDS
}

fn is_loopback_url(url: &str) -> bool {
    url.starts_with("http://127.") || url.starts_with("http://localhost:") || url.starts_with("http://[::1]")
}

// The library sends DKG secret shares over the transport in the clear, so plain HTTP is only safe on loopback.
// Remove this guard only together with mutual TLS.
pub fn refuse_plaintext_beyond_loopback(config: &SignerConfig) -> Result<(), String> {
    if !config.listen_address.starts_with("127.") && !config.listen_address.starts_with("[::1]") {
        return Err(format!("listen_address {} is not loopback and mutual TLS is not built", config.listen_address));
    }
    for peer in &config.peers {
        if !is_loopback_url(&peer.url) {
            return Err(format!("peer {} url {} is not loopback and mutual TLS is not built", peer.signer_id, peer.url));
        }
    }
    Ok(())
}

pub fn load_signer_config(path: &str) -> Result<SignerConfig, String> {
    let raw_config = std::fs::read(path).map_err(|error| format!("read config {path}: {error}"))?;
    let config: SignerConfig =
        serde_json::from_slice(&raw_config).map_err(|error| format!("parse config {path}: {error}"))?;
    refuse_plaintext_beyond_loopback(&config)?;
    Ok(config)
}
