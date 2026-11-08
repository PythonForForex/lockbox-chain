use crate::nonce_record::write_file_durably;
use crate::order_intent::sha256_hex;
use crate::threshold_protocol::{venue_address_of_share, LockboxKeyShare};
use serde::{Deserialize, Serialize};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const OWNER_READ_WRITE_ONLY: u32 = 0o600;

#[derive(Serialize, Deserialize)]
struct KeyShareFile {
    key_ref: String,
    committee: Vec<String>,
    threshold: u16,
    key_share: LockboxKeyShare,
}

pub struct StoredKeyShare {
    pub key_ref: String,
    pub committee: Vec<String>,
    pub threshold: u16,
    pub venue_address: String,
    pub key_share: LockboxKeyShare,
}

fn committed_share_path(state_directory: &Path, key_ref: &str) -> PathBuf {
    state_directory.join("key_shares").join(format!("{}.share", sha256_hex(key_ref.as_bytes())))
}

fn pending_share_path(state_directory: &Path, key_ref: &str) -> PathBuf {
    state_directory.join("key_shares").join(format!("{}.pending.share", sha256_hex(key_ref.as_bytes())))
}

pub fn committed_share_exists(state_directory: &Path, key_ref: &str) -> bool {
    committed_share_path(state_directory, key_ref).exists()
}

pub fn write_pending_share(
    state_directory: &Path,
    key_ref: &str,
    committee: &[String],
    threshold: u16,
    key_share: &LockboxKeyShare,
) -> Result<(), String> {
    let file = serde_json::json!({
        "key_ref": key_ref,
        "committee": committee,
        "threshold": threshold,
        "key_share": key_share,
    });
    let target_path = pending_share_path(state_directory, key_ref);
    write_file_durably(&target_path, file.to_string().as_bytes())?;
    fs::set_permissions(&target_path, fs::Permissions::from_mode(OWNER_READ_WRITE_ONLY)).map_err(|error| error.to_string())
}

pub fn commit_pending_share(state_directory: &Path, key_ref: &str) -> Result<(), String> {
    let committed_path = committed_share_path(state_directory, key_ref);
    if committed_path.exists() {
        return Err("a share for this key_ref already exists and is never overwritten".to_string());
    }
    fs::rename(pending_share_path(state_directory, key_ref), &committed_path).map_err(|error| error.to_string())?;
    let directory = committed_path.parent().ok_or("share path has no parent")?;
    fs::File::open(directory).and_then(|handle| handle.sync_all()).map_err(|error| error.to_string())
}

fn stored_share_from_file(file: KeyShareFile) -> StoredKeyShare {
    StoredKeyShare {
        venue_address: venue_address_of_share(&file.key_share),
        key_ref: file.key_ref,
        committee: file.committee,
        threshold: file.threshold,
        key_share: file.key_share,
    }
}

pub fn load_committed_share(state_directory: &Path, key_ref: &str) -> Result<Option<Arc<StoredKeyShare>>, String> {
    let committed_path = committed_share_path(state_directory, key_ref);
    let raw_file = match fs::read(&committed_path) {
        Ok(raw_file) => raw_file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("key share unreadable: {error}")),
    };
    let file: KeyShareFile = serde_json::from_slice(&raw_file).map_err(|error| format!("key share corrupt: {error}"))?;
    if file.key_ref != key_ref {
        return Err("key share file holds another key_ref".to_string());
    }
    Ok(Some(Arc::new(stored_share_from_file(file))))
}
