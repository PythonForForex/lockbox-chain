use crate::order_intent::{parse_positive_decimal_string, OrderIntent};
use serde_json::{json, Value};
use sha3::{Digest, Keccak256};

const MAXIMUM_WIRE_DECIMAL_PLACES: usize = 8;
const EXCHANGE_DOMAIN_CHAIN_ID: u64 = 1337;
const DOMAIN_TYPE: &[u8] = b"EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)";
const AGENT_TYPE: &[u8] = b"Agent(string source,bytes32 connectionId)";
const EXCHANGE_DOMAIN_NAME: &[u8] = b"Exchange";
const EXCHANGE_DOMAIN_VERSION: &[u8] = b"1";
const MAINNET_SOURCE: &[u8] = b"a";
const MILLISECONDS_PER_SECOND: u64 = 1000;

pub struct BuiltAction {
    pub action: Value,
    pub digest: [u8; 32],
}

pub fn keccak256(data: &[u8]) -> [u8; 32] {
    Keccak256::digest(data).into()
}

// Hyperliquid hashes the action with prices and sizes in canonical form; a trailing zero gives a signature for a
// different hash and the venue recovers some other address. The SDK's float_to_wire allows at most 8 decimals.
pub fn decimal_string_to_wire(field_name: &str, text: &str) -> Result<String, String> {
    if parse_positive_decimal_string(text).is_none() {
        return Err(format!("bad_{field_name}"));
    }
    let Some((whole_part, fraction_part)) = text.split_once('.') else {
        return Ok(text.to_string());
    };
    let trimmed_fraction = fraction_part.trim_end_matches('0');
    if trimmed_fraction.len() > MAXIMUM_WIRE_DECIMAL_PLACES {
        return Err(format!("bad_{field_name}"));
    }
    if trimmed_fraction.is_empty() {
        return Ok(whole_part.to_string());
    }
    Ok(format!("{whole_part}.{trimmed_fraction}"))
}

fn msgpack_write_string(text: &str, output: &mut Vec<u8>) {
    let length = text.len();
    if length < 32 {
        output.push(0xa0 | length as u8);
    } else if length < 0x100 {
        output.extend_from_slice(&[0xd9, length as u8]);
    } else if length < 0x10000 {
        output.push(0xda);
        output.extend_from_slice(&(length as u16).to_be_bytes());
    } else {
        output.push(0xdb);
        output.extend_from_slice(&(length as u32).to_be_bytes());
    }
    output.extend_from_slice(text.as_bytes());
}

fn msgpack_write_unsigned(value: u64, output: &mut Vec<u8>) {
    if value < 0x80 {
        output.push(value as u8);
    } else if value < 0x100 {
        output.extend_from_slice(&[0xcc, value as u8]);
    } else if value < 0x10000 {
        output.push(0xcd);
        output.extend_from_slice(&(value as u16).to_be_bytes());
    } else if value < 0x1_0000_0000 {
        output.push(0xce);
        output.extend_from_slice(&(value as u32).to_be_bytes());
    } else {
        output.push(0xcf);
        output.extend_from_slice(&value.to_be_bytes());
    }
}

fn msgpack_write_collection_header(length: usize, small_base: u8, code16: u8, code32: u8, output: &mut Vec<u8>) {
    if length < 16 {
        output.push(small_base | length as u8);
    } else if length < 0x10000 {
        output.push(code16);
        output.extend_from_slice(&(length as u16).to_be_bytes());
    } else {
        output.push(code32);
        output.extend_from_slice(&(length as u32).to_be_bytes());
    }
}

// Key order is the action's own insertion order: Hyperliquid re-serialises in the SDK's order and hashes that.
pub fn msgpack_write_value(value: &Value, output: &mut Vec<u8>) -> Result<(), String> {
    match value {
        Value::Bool(flag) => output.push(if *flag { 0xc3 } else { 0xc2 }),
        Value::String(text) => msgpack_write_string(text, output),
        Value::Number(number) => {
            msgpack_write_unsigned(number.as_u64().ok_or("action number is not an unsigned integer")?, output)
        }
        Value::Array(items) => {
            msgpack_write_collection_header(items.len(), 0x90, 0xdc, 0xdd, output);
            for item in items {
                msgpack_write_value(item, output)?;
            }
        }
        Value::Object(object) => {
            msgpack_write_collection_header(object.len(), 0x80, 0xde, 0xdf, output);
            for (key, entry) in object {
                msgpack_write_string(key, output);
                msgpack_write_value(entry, output)?;
            }
        }
        Value::Null => return Err("action holds a null".to_string()),
    }
    Ok(())
}

fn exchange_domain_separator() -> [u8; 32] {
    let mut chain_id_word = [0u8; 32];
    chain_id_word[24..].copy_from_slice(&EXCHANGE_DOMAIN_CHAIN_ID.to_be_bytes());
    let mut encoded = Vec::with_capacity(32 * 5);
    encoded.extend_from_slice(&keccak256(DOMAIN_TYPE));
    encoded.extend_from_slice(&keccak256(EXCHANGE_DOMAIN_NAME));
    encoded.extend_from_slice(&keccak256(EXCHANGE_DOMAIN_VERSION));
    encoded.extend_from_slice(&chain_id_word);
    encoded.extend_from_slice(&[0u8; 32]);
    keccak256(&encoded)
}

// Same bytes as Hyperliquid's own action_hash: the action, the nonce, 0x00 or 0x01 plus the vault, then
// 0x00 plus expiresAfter in milliseconds. Hyperliquid recomputes this, so the same two values go in the posted body.
pub fn phantom_agent_digest(action_bytes: &[u8], venue_nonce: u64, vault_address: Option<&[u8; 20]>, expires_after_milliseconds: u64) -> [u8; 32] {
    let mut connection_preimage = Vec::with_capacity(action_bytes.len() + 38);
    connection_preimage.extend_from_slice(action_bytes);
    connection_preimage.extend_from_slice(&venue_nonce.to_be_bytes());
    match vault_address {
        None => connection_preimage.push(0),
        Some(vault_bytes) => {
            connection_preimage.push(1);
            connection_preimage.extend_from_slice(vault_bytes);
        }
    }
    connection_preimage.push(0);
    connection_preimage.extend_from_slice(&expires_after_milliseconds.to_be_bytes());
    let connection_id = keccak256(&connection_preimage);

    let mut agent_preimage = Vec::with_capacity(32 * 3);
    agent_preimage.extend_from_slice(&keccak256(AGENT_TYPE));
    agent_preimage.extend_from_slice(&keccak256(MAINNET_SOURCE));
    agent_preimage.extend_from_slice(&connection_id);
    let agent_struct_hash = keccak256(&agent_preimage);

    let mut typed_data = Vec::with_capacity(66);
    typed_data.extend_from_slice(b"\x19\x01");
    typed_data.extend_from_slice(&exchange_domain_separator());
    typed_data.extend_from_slice(&agent_struct_hash);
    keccak256(&typed_data)
}

fn build_action_value(intent: &OrderIntent) -> Result<Value, String> {
    match intent.kind.as_str() {
        "order" => {
            let tif = intent.tif.as_deref().unwrap_or_default();
            if !matches!(tif, "Alo" | "Ioc" | "Gtc") {
                return Err("bad_tif".to_string());
            }
            let wire_price = decimal_string_to_wire("limit_px", intent.limit_px.as_deref().unwrap_or_default())?;
            let wire_size = decimal_string_to_wire("size", intent.size.as_deref().unwrap_or_default())?;
            Ok(json!({
                "type": "order",
                "orders": [{
                    "a": intent.asset_index,
                    "b": intent.is_buy,
                    "p": wire_price,
                    "s": wire_size,
                    "r": intent.reduce_only,
                    "t": {"limit": {"tif": tif}},
                }],
                "grouping": "na",
            }))
        }
        "cancel" => Ok(json!({"type": "cancel", "cancels": [{"a": intent.asset_index, "o": intent.oid}]})),
        "schedule_cancel" => Ok(json!({"type": "scheduleCancel", "time": intent.time_ms})),
        _ => Err("unknown_kind".to_string()),
    }
}

fn vault_address_bytes(vault_address: &str) -> Result<Option<[u8; 20]>, String> {
    if vault_address.is_empty() {
        return Ok(None);
    }
    let decoded = hex::decode(vault_address.trim_start_matches("0x")).map_err(|_| "bad_vault_address")?;
    let vault_bytes: [u8; 20] = decoded.try_into().map_err(|_| "bad_vault_address")?;
    Ok(Some(vault_bytes))
}

pub fn build_action_and_digest(intent: &OrderIntent) -> Result<BuiltAction, String> {
    let action = build_action_value(intent)?;
    let mut action_bytes = Vec::new();
    msgpack_write_value(&action, &mut action_bytes)?;
    let vault_bytes = vault_address_bytes(&intent.vault_address)?;
    let expires_after_milliseconds = intent.expires_at.checked_mul(MILLISECONDS_PER_SECOND).ok_or("intent_expiry_too_far")?;
    let digest = phantom_agent_digest(&action_bytes, intent.venue_nonce, vault_bytes.as_ref(), expires_after_milliseconds);
    Ok(BuiltAction { action, digest })
}
