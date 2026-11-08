use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

const NODE_REQUEST_TIMEOUT_MILLISECONDS: u64 = 1500;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChainAssetCap {
    pub asset: String,
    pub max_order_size: String,
    pub max_daily_size: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChainPolicy {
    pub epoch: u64,
    pub venues: Vec<String>,
    pub vault_address: String,
    pub asset_caps: Vec<ChainAssetCap>,
    pub expires_at: u64,
    pub revoked: bool,
    pub revoked_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChainKeyRef {
    pub venue_address: String,
    pub committee: Vec<String>,
    pub threshold: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct ChainStatus {
    pub latest_block_height: u64,
    pub latest_block_time_seconds: u64,
    pub catching_up: bool,
}

pub fn build_node_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_millis(NODE_REQUEST_TIMEOUT_MILLISECONDS))
        .build()
        .expect("http client builds")
}

async fn fetch_json(http_client: &reqwest::Client, url: &str) -> Result<Value, String> {
    let response = http_client.get(url).send().await.map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!("{url} returned {}", response.status()));
    }
    response.json::<Value>().await.map_err(|error| error.to_string())
}

// Cosmos REST renders uint64 as a string and wraps a query result in a field named after the item.
fn read_unsigned(object: &Value, field_name: &str) -> Result<u64, String> {
    match &object[field_name] {
        Value::Number(number) => number.as_u64().ok_or(format!("{field_name} is not unsigned")),
        Value::String(text) => text.parse().map_err(|_| format!("{field_name} is not unsigned")),
        Value::Null => Ok(0),
        _ => Err(format!("{field_name} is not unsigned")),
    }
}

fn read_string_list(object: &Value, field_name: &str) -> Vec<String> {
    object[field_name]
        .as_array()
        .map(|items| items.iter().filter_map(|item| item.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

fn unwrap_named_field<'a>(document: &'a Value, field_name: &str) -> &'a Value {
    if document[field_name].is_object() {
        &document[field_name]
    } else {
        document
    }
}

pub async fn read_policy(
    http_client: &reqwest::Client,
    chain_rest_url: &str,
    key_ref: &str,
    delegate_key: &str,
) -> Result<ChainPolicy, String> {
    let url = format!("{chain_rest_url}/lockbox/v1/policy/{key_ref}/{delegate_key}");
    let document = fetch_json(http_client, &url).await?;
    let policy = unwrap_named_field(&document, "policy");
    Ok(ChainPolicy {
        epoch: read_unsigned(policy, "epoch")?,
        venues: read_string_list(policy, "venues"),
        vault_address: policy["vault_address"].as_str().unwrap_or_default().to_string(),
        asset_caps: serde_json::from_value(policy["asset_caps"].clone()).map_err(|error| format!("asset_caps: {error}"))?,
        expires_at: read_unsigned(policy, "expires_at")?,
        revoked: policy["revoked"].as_bool().unwrap_or(false),
        revoked_at: read_unsigned(policy, "revoked_at")?,
    })
}

pub async fn read_key_ref(
    http_client: &reqwest::Client,
    chain_rest_url: &str,
    key_ref: &str,
) -> Result<ChainKeyRef, String> {
    let url = format!("{chain_rest_url}/lockbox/v1/keyref/{key_ref}");
    let document = fetch_json(http_client, &url).await?;
    let key_reference = unwrap_named_field(&document, "key_ref");
    Ok(ChainKeyRef {
        venue_address: key_reference["venue_address"].as_str().unwrap_or_default().to_string(),
        committee: read_string_list(key_reference, "committee"),
        threshold: read_unsigned(key_reference, "threshold")? as u32,
    })
}

fn days_from_civil_date(year: i64, month: u64, day: u64) -> i64 {
    let shifted_year = if month <= 2 { year - 1 } else { year };
    let era = shifted_year.div_euclid(400);
    let year_of_era = shifted_year.rem_euclid(400) as u64;
    let shifted_month = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146097 + day_of_era as i64 - 719468
}

// CometBFT gives RFC 3339 UTC, for example 2026-11-08T01:02:03.123456789Z.
pub fn parse_block_time_seconds(text: &str) -> Result<u64, String> {
    let bytes = text.as_bytes();
    let is_well_formed = bytes.len() >= 20 && bytes[4] == b'-' && bytes[7] == b'-' && bytes[10] == b'T' && bytes[13] == b':' && bytes[16] == b':';
    if !is_well_formed || !text.ends_with('Z') {
        return Err(format!("block time {text} is not RFC 3339 UTC"));
    }
    let number = |range: std::ops::Range<usize>| -> Result<u64, String> {
        text[range].parse::<u64>().map_err(|_| format!("block time {text} is not RFC 3339 UTC"))
    };
    let days = days_from_civil_date(number(0..4)? as i64, number(5..7)?, number(8..10)?);
    if days < 0 {
        return Err(format!("block time {text} predates 1970"));
    }
    Ok(days as u64 * 86400 + number(11..13)? * 3600 + number(14..16)? * 60 + number(17..19)?)
}

pub async fn read_chain_status(http_client: &reqwest::Client, chain_rpc_url: &str) -> Result<ChainStatus, String> {
    let document = fetch_json(http_client, &format!("{chain_rpc_url}/status")).await?;
    let result = unwrap_named_field(&document, "result");
    let sync_info = &result["sync_info"];
    Ok(ChainStatus {
        latest_block_height: read_unsigned(sync_info, "latest_block_height")?,
        latest_block_time_seconds: parse_block_time_seconds(sync_info["latest_block_time"].as_str().unwrap_or_default())?,
        catching_up: sync_info["catching_up"].as_bool().unwrap_or(false),
    })
}
