use crate::order_intent::sha256_hex;
use crate::policy_rules::NewRiskSize;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;

const SECONDS_PER_DAY: u64 = 86_400;
const MILLISECONDS_PER_SECOND: u64 = 1000;
// Hyperliquid accepts a nonce at most 2 days old; older can never fill, so it must not reach the record.
const VENUE_NONCE_MAX_AGE_MILLISECONDS: u64 = 2 * SECONDS_PER_DAY * MILLISECONDS_PER_SECOND;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DailySize {
    pub utc_day: u64,
    pub size: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct KeyNonceRecord {
    pub key_ref: String,
    pub highest_signed_nonce: u64,
    pub daily_size_by_delegate_and_asset: BTreeMap<String, DailySize>,
}

fn daily_size_key(delegate_key: &str, asset: &str) -> String {
    format!("{delegate_key}/{asset}")
}

pub fn size_spent_today(record: &KeyNonceRecord, delegate_key: &str, asset: &str, now_seconds: u64) -> Decimal {
    record
        .daily_size_by_delegate_and_asset
        .get(&daily_size_key(delegate_key, asset))
        .filter(|daily| daily.utc_day == now_seconds / SECONDS_PER_DAY)
        .and_then(|daily| Decimal::from_str(&daily.size).ok())
        .unwrap_or(Decimal::ZERO)
}

pub fn check_venue_nonce_window(venue_nonce: u64, now_seconds: u64, max_ahead_milliseconds: u64) -> Result<(), String> {
    let now_milliseconds = now_seconds.saturating_mul(MILLISECONDS_PER_SECOND);
    if venue_nonce < now_milliseconds.saturating_sub(VENUE_NONCE_MAX_AGE_MILLISECONDS)
        || venue_nonce > now_milliseconds.saturating_add(max_ahead_milliseconds)
    {
        return Err("venue_nonce_outside_window".to_string());
    }
    Ok(())
}

fn record_file_path(state_directory: &Path, key_ref: &str) -> PathBuf {
    state_directory.join("nonce_records").join(format!("{}.json", sha256_hex(key_ref.as_bytes())))
}

pub fn load_record(state_directory: &Path, key_ref: &str) -> Result<KeyNonceRecord, String> {
    match fs::read(record_file_path(state_directory, key_ref)) {
        Ok(raw_record) => serde_json::from_slice(&raw_record).map_err(|error| format!("nonce record corrupt: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(KeyNonceRecord { key_ref: key_ref.to_string(), ..KeyNonceRecord::default() })
        }
        Err(error) => Err(format!("nonce record unreadable: {error}")),
    }
}

// Write, fsync the file, rename over the old one, fsync the directory: after a crash the record is either the old
// one or the new one, never a partial file.
pub fn write_file_durably(target_path: &Path, contents: &[u8]) -> Result<(), String> {
    let directory = target_path.parent().ok_or("target has no parent directory")?;
    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let temporary_path = target_path.with_extension(format!("tmp{}", std::process::id()));
    let mut temporary_file = File::create(&temporary_path).map_err(|error| error.to_string())?;
    temporary_file.write_all(contents).map_err(|error| error.to_string())?;
    temporary_file.sync_all().map_err(|error| error.to_string())?;
    drop(temporary_file);
    fs::rename(&temporary_path, target_path).map_err(|error| error.to_string())?;
    File::open(directory).and_then(|handle| handle.sync_all()).map_err(|error| error.to_string())
}

pub fn persist_record(state_directory: &Path, record: &KeyNonceRecord) -> Result<(), String> {
    let serialized_record = serde_json::to_vec(record).map_err(|error| error.to_string())?;
    write_file_durably(&record_file_path(state_directory, &record.key_ref), &serialized_record)
}

pub fn check_and_reserve(
    record: &KeyNonceRecord,
    venue_nonce: u64,
    delegate_key: &str,
    new_risk_size: Option<&NewRiskSize>,
    now_seconds: u64,
) -> Result<KeyNonceRecord, String> {
    if venue_nonce <= record.highest_signed_nonce {
        return Err(format!("nonce_not_above_highest_signed_{}", record.highest_signed_nonce));
    }
    let mut reserved_record = record.clone();
    reserved_record.highest_signed_nonce = venue_nonce;
    if let Some(new_risk) = new_risk_size {
        let spent = size_spent_today(record, delegate_key, &new_risk.asset, now_seconds);
        let total = spent.checked_add(new_risk.size).ok_or("over_max_daily_size")?;
        reserved_record.daily_size_by_delegate_and_asset.insert(
            daily_size_key(delegate_key, &new_risk.asset),
            DailySize { utc_day: now_seconds / SECONDS_PER_DAY, size: total.normalize().to_string() },
        );
    }
    Ok(reserved_record)
}

fn merge_daily_size(merged_record: &mut KeyNonceRecord, size_key: &str, peer_daily: &DailySize) {
    let Ok(peer_size) = Decimal::from_str(&peer_daily.size) else {
        return;
    };
    let replaces_own = match merged_record.daily_size_by_delegate_and_asset.get(size_key) {
        None => true,
        Some(own_daily) if peer_daily.utc_day > own_daily.utc_day => true,
        Some(own_daily) if peer_daily.utc_day == own_daily.utc_day => {
            Decimal::from_str(&own_daily.size).map(|own| peer_size > own).unwrap_or(true)
        }
        Some(_) => false,
    };
    if replaces_own {
        merged_record.daily_size_by_delegate_and_asset.insert(size_key.to_string(), peer_daily.clone());
    }
}

// A peer can claim any number, so a claim beyond the allowed nonce window is ignored: otherwise one bad peer could
// jam this key for ever by claiming u64::MAX.
pub fn merge_peer_record(
    own_record: &KeyNonceRecord,
    peer_record: &KeyNonceRecord,
    now_seconds: u64,
    max_ahead_milliseconds: u64,
) -> Result<KeyNonceRecord, String> {
    let now_milliseconds = now_seconds.saturating_mul(MILLISECONDS_PER_SECOND);
    if peer_record.highest_signed_nonce > now_milliseconds.saturating_add(max_ahead_milliseconds) {
        return Err(format!("claimed nonce {} beyond the window", peer_record.highest_signed_nonce));
    }
    let mut merged_record = own_record.clone();
    merged_record.highest_signed_nonce = own_record.highest_signed_nonce.max(peer_record.highest_signed_nonce);
    for (size_key, peer_daily) in &peer_record.daily_size_by_delegate_and_asset {
        merge_daily_size(&mut merged_record, size_key, peer_daily);
    }
    Ok(merged_record)
}
