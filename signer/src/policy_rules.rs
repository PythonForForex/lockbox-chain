use crate::order_intent::{parse_positive_decimal_string, OrderIntent};
use crate::policy_reader::{ChainAssetCap, ChainPolicy};
use rust_decimal::Decimal;
use std::collections::BTreeMap;

const SUPPORTED_VENUE: &str = "hyperliquid";
const MAXIMUM_INTENT_LIFETIME_SECONDS: u64 = 120;
const MINIMUM_SCHEDULE_CANCEL_AHEAD_SECONDS: u64 = 15;
const MAXIMUM_SCHEDULE_CANCEL_AHEAD_SECONDS: u64 = 600;
const CLOSE_OUT_SECONDS: u64 = 600;
const MILLISECONDS_PER_SECOND: u64 = 1000;

#[derive(Clone, Copy, Debug)]
pub struct ChainFreshness {
    pub now_seconds: u64,
    pub node_reachable: bool,
    pub node_catching_up: bool,
    pub latest_block_time_seconds: u64,
    pub local_height: u64,
    pub highest_known_height: u64,
    pub max_blocks_behind: u64,
    pub halt_timeout_seconds: u64,
}

pub fn is_chain_stale(freshness: &ChainFreshness) -> bool {
    !freshness.node_reachable
        || freshness.node_catching_up
        || freshness.highest_known_height.saturating_sub(freshness.local_height) > freshness.max_blocks_behind
        || freshness.now_seconds > freshness.latest_block_time_seconds + freshness.halt_timeout_seconds
}

pub struct NewRiskSize {
    pub asset: String,
    pub size: Decimal,
}

pub struct AllowedIntent {
    pub new_risk_size: Option<NewRiskSize>,
}

const TAKES_NO_NEW_RISK: AllowedIntent = AllowedIntent { new_risk_size: None };

fn close_out_deadline_seconds(policy: &ChainPolicy) -> u64 {
    let ended_at = if policy.revoked { policy.revoked_at.min(policy.expires_at) } else { policy.expires_at };
    ended_at + CLOSE_OUT_SECONDS
}

fn evaluate_new_risk(intent: &OrderIntent, asset_cap: &ChainAssetCap, size_spent_today: Decimal) -> Result<AllowedIntent, String> {
    let asset_name = intent.asset.as_deref().unwrap_or_default();
    parse_positive_decimal_string(intent.limit_px.as_deref().unwrap_or_default()).ok_or("bad_limit_px")?;
    let size = parse_positive_decimal_string(intent.size.as_deref().unwrap_or_default()).ok_or("bad_size")?;
    let max_order_size = parse_positive_decimal_string(&asset_cap.max_order_size).ok_or("bad_policy_cap")?;
    let max_daily_size = parse_positive_decimal_string(&asset_cap.max_daily_size).ok_or("bad_policy_cap")?;
    if size > max_order_size {
        return Err("over_max_order_size".to_string());
    }
    let size_after_order = size_spent_today.checked_add(size).ok_or("over_max_daily_size")?;
    if size_after_order > max_daily_size {
        return Err("over_max_daily_size".to_string());
    }
    Ok(AllowedIntent { new_risk_size: Some(NewRiskSize { asset: asset_name.to_string(), size }) })
}

// An empty time removes Hyperliquid's dead-man, so time_ms is mandatory, and a far time would defeat it.
fn check_schedule_cancel_time(intent: &OrderIntent, now_seconds: u64, deadline_seconds: u64) -> Result<(), String> {
    let time_ms = intent.time_ms.ok_or("schedule_cancel_time_missing")?;
    let earliest = (now_seconds + MINIMUM_SCHEDULE_CANCEL_AHEAD_SECONDS) * MILLISECONDS_PER_SECOND;
    let latest = (now_seconds + MAXIMUM_SCHEDULE_CANCEL_AHEAD_SECONDS) * MILLISECONDS_PER_SECOND;
    if time_ms < earliest || time_ms > latest {
        return Err("schedule_cancel_time_out_of_range".to_string());
    }
    if time_ms > deadline_seconds * MILLISECONDS_PER_SECOND {
        return Err("schedule_cancel_past_close_out_deadline".to_string());
    }
    Ok(())
}

// A delegate gets a bounded time to close out: until the earlier of revoked_at and expires_at plus 600 s, cancels and
// schedule_cancel stay open at any epoch, because they only take risk off. An order, reduce-only included, stops at once.
// After the deadline nothing from this delegate is signed.
pub fn evaluate_intent(
    intent: &OrderIntent,
    policy: &ChainPolicy,
    freshness: &ChainFreshness,
    asset_index_by_name: &BTreeMap<String, u32>,
    size_spent_today: Decimal,
) -> Result<AllowedIntent, String> {
    if freshness.now_seconds >= intent.expires_at {
        return Err("intent_expired".to_string());
    }
    if intent.expires_at > freshness.now_seconds + MAXIMUM_INTENT_LIFETIME_SECONDS {
        return Err("intent_expiry_too_far".to_string());
    }
    if intent.venue != SUPPORTED_VENUE || !policy.venues.iter().any(|venue| venue == SUPPORTED_VENUE) {
        return Err("venue_not_allowed".to_string());
    }
    if intent.vault_address != policy.vault_address {
        return Err("vault_mismatch".to_string());
    }
    let deadline_seconds = close_out_deadline_seconds(policy);
    if freshness.now_seconds >= deadline_seconds {
        return Err("policy_closed".to_string());
    }
    match intent.kind.as_str() {
        "cancel" => return Ok(TAKES_NO_NEW_RISK),
        "schedule_cancel" => {
            check_schedule_cancel_time(intent, freshness.now_seconds, deadline_seconds)?;
            return Ok(TAKES_NO_NEW_RISK);
        }
        _ => {}
    }
    if policy.revoked {
        return Err("policy_revoked".to_string());
    }
    if freshness.now_seconds >= policy.expires_at {
        return Err("policy_expired".to_string());
    }
    if intent.policy_epoch != policy.epoch {
        return Err(format!("epoch_mismatch_policy_is_{}", policy.epoch));
    }
    // The action carries asset_index and the policy names assets, so a delegate could name an allowed asset and send
    // the index of another. The name must map to that index in this signer's own table.
    let asset_name = intent.asset.as_deref().unwrap_or_default();
    if asset_index_by_name.get(asset_name) != intent.asset_index.as_ref() || intent.asset_index.is_none() {
        return Err("asset_index_does_not_match_asset".to_string());
    }
    let asset_cap = policy.asset_caps.iter().find(|asset_cap| asset_cap.asset == asset_name).ok_or("asset_not_allowed")?;
    if intent.reduce_only == Some(true) {
        return Ok(TAKES_NO_NEW_RISK);
    }
    if is_chain_stale(freshness) {
        return Err("chain_stale_new_risk_stopped".to_string());
    }
    evaluate_new_risk(intent, asset_cap, size_spent_today)
}
