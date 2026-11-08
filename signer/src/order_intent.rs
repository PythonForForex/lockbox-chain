use ed25519_dalek::{Signature, VerifyingKey};
use rust_decimal::Decimal;
use serde::de::{Deserializer, MapAccess, SeqAccess, Visitor};
use serde::Deserialize;
use serde_json::{Map, Number, Value};
use sha2::{Digest, Sha256};
use std::fmt;
use std::str::FromStr;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrderIntent {
    pub request_id: String,
    pub key_ref: String,
    pub delegate_key: String,
    pub policy_epoch: u64,
    pub venue: String,
    pub vault_address: String,
    pub kind: String,
    pub venue_nonce: u64,
    pub expires_at: u64,
    pub delegate_signature: String,
    pub asset_index: Option<u32>,
    pub asset: Option<String>,
    pub is_buy: Option<bool>,
    pub limit_px: Option<String>,
    pub size: Option<String>,
    pub reduce_only: Option<bool>,
    pub tif: Option<String>,
    pub oid: Option<u64>,
    pub time_ms: Option<u64>,
}

pub struct VerifiedIntent {
    pub intent: OrderIntent,
    pub intent_hash: String,
}

pub struct StrictJsonValue(pub Value);

struct StrictJsonVisitor;

impl<'de> Visitor<'de> for StrictJsonVisitor {
    type Value = StrictJsonValue;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("JSON without duplicate keys or floats")
    }

    fn visit_bool<E>(self, value: bool) -> Result<StrictJsonValue, E> {
        Ok(StrictJsonValue(Value::Bool(value)))
    }

    fn visit_u64<E>(self, value: u64) -> Result<StrictJsonValue, E> {
        Ok(StrictJsonValue(Value::Number(Number::from(value))))
    }

    fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<StrictJsonValue, E> {
        Err(E::custom(format!("negative number {value}")))
    }

    fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<StrictJsonValue, E> {
        Err(E::custom(format!("float {value}: prices and sizes are decimal strings")))
    }

    fn visit_str<E>(self, value: &str) -> Result<StrictJsonValue, E> {
        Ok(StrictJsonValue(Value::String(value.to_string())))
    }

    fn visit_unit<E>(self) -> Result<StrictJsonValue, E> {
        Ok(StrictJsonValue(Value::Null))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<StrictJsonValue, A::Error> {
        let mut items = Vec::new();
        while let Some(StrictJsonValue(item)) = sequence.next_element()? {
            items.push(item);
        }
        Ok(StrictJsonValue(Value::Array(items)))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut entries: A) -> Result<StrictJsonValue, A::Error> {
        let mut object = Map::new();
        while let Some((key, StrictJsonValue(value))) = entries.next_entry::<String, StrictJsonValue>()? {
            if object.contains_key(&key) {
                return Err(serde::de::Error::custom(format!("duplicate key {key}")));
            }
            object.insert(key, value);
        }
        Ok(StrictJsonValue(Value::Object(object)))
    }
}

impl<'de> Deserialize<'de> for StrictJsonValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(StrictJsonVisitor)
    }
}

pub fn parse_strict_json(raw_bytes: &[u8]) -> Result<Value, String> {
    serde_json::from_slice::<StrictJsonValue>(raw_bytes)
        .map(|strict_value| strict_value.0)
        .map_err(|error| error.to_string())
}

// Printable ASCII only: Go, Python and Rust JSON encoders escape everything else differently, and the delegate
// signature covers the encoded bytes.
fn write_canonical_string(text: &str, output: &mut String) -> Result<(), String> {
    if text.chars().any(|character| !(' '..='~').contains(&character)) {
        return Err("string with non printable or non ASCII characters".to_string());
    }
    output.push('"');
    for character in text.chars() {
        if character == '"' || character == '\\' {
            output.push('\\');
        }
        output.push(character);
    }
    output.push('"');
    Ok(())
}

fn write_canonical_value(value: &Value, output: &mut String) -> Result<(), String> {
    match value {
        Value::Null => output.push_str("null"),
        Value::Bool(flag) => output.push_str(if *flag { "true" } else { "false" }),
        Value::Number(number) => output.push_str(&number.to_string()),
        Value::String(text) => write_canonical_string(text, output)?,
        Value::Array(items) => {
            output.push('[');
            for (position, item) in items.iter().enumerate() {
                if position > 0 {
                    output.push(',');
                }
                write_canonical_value(item, output)?;
            }
            output.push(']');
        }
        Value::Object(object) => {
            let mut keys: Vec<&String> = object.keys().collect();
            keys.sort();
            output.push('{');
            for (position, key) in keys.iter().enumerate() {
                if position > 0 {
                    output.push(',');
                }
                write_canonical_string(key, output)?;
                output.push(':');
                write_canonical_value(&object[*key], output)?;
            }
            output.push('}');
        }
    }
    Ok(())
}

pub fn canonical_intent_bytes(raw_intent: &Value) -> Result<Vec<u8>, String> {
    let mut object = raw_intent.as_object().ok_or("intent is not an object")?.clone();
    object.shift_remove("delegate_signature");
    let mut output = String::new();
    write_canonical_value(&Value::Object(object), &mut output)?;
    Ok(output.into_bytes())
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

pub fn is_valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.chars().all(|character| character.is_ascii_alphanumeric() || "._:-".contains(character))
}

fn is_lowercase_hex_of_length(value: &str, length: usize) -> bool {
    value.len() == length && value.chars().all(|character| matches!(character, '0'..='9' | 'a'..='f'))
}

fn refuse_fields_of_other_kinds(intent: &OrderIntent) -> Result<(), String> {
    let has_order_fields = intent.is_buy.is_some()
        || intent.limit_px.is_some()
        || intent.size.is_some()
        || intent.reduce_only.is_some()
        || intent.tif.is_some();
    match intent.kind.as_str() {
        "order" => {
            if intent.oid.is_some() || intent.time_ms.is_some() {
                return Err("order carries oid or time_ms".to_string());
            }
            if intent.asset_index.is_none()
                || intent.asset.is_none()
                || intent.is_buy.is_none()
                || intent.limit_px.is_none()
                || intent.size.is_none()
                || intent.reduce_only.is_none()
                || intent.tif.is_none()
            {
                return Err("order is missing a field".to_string());
            }
        }
        "cancel" => {
            if has_order_fields || intent.time_ms.is_some() {
                return Err("cancel carries order fields".to_string());
            }
            if intent.asset_index.is_none() || intent.oid.is_none() {
                return Err("cancel is missing asset_index or oid".to_string());
            }
        }
        "schedule_cancel" => {
            if has_order_fields || intent.oid.is_some() || intent.asset_index.is_some() || intent.asset.is_some() {
                return Err("schedule_cancel carries order fields".to_string());
            }
            if intent.time_ms.is_none() {
                return Err("schedule_cancel_time_missing".to_string());
            }
        }
        _ => return Err("unknown_kind".to_string()),
    }
    Ok(())
}

pub fn verify_delegate_signature(raw_intent: &Value, intent: &OrderIntent) -> Result<Vec<u8>, String> {
    let canonical_bytes = canonical_intent_bytes(raw_intent)?;
    let key_bytes: [u8; 32] = hex::decode(&intent.delegate_key)
        .map_err(|_| "bad_delegate_key")?
        .try_into()
        .map_err(|_| "bad_delegate_key")?;
    let signature_bytes: [u8; 64] = hex::decode(&intent.delegate_signature)
        .map_err(|_| "bad_delegate_signature")?
        .try_into()
        .map_err(|_| "bad_delegate_signature")?;
    let verifying_key = VerifyingKey::from_bytes(&key_bytes).map_err(|_| "bad_delegate_key")?;
    verifying_key
        .verify_strict(&canonical_bytes, &Signature::from_bytes(&signature_bytes))
        .map_err(|_| "bad_delegate_signature")?;
    Ok(canonical_bytes)
}

pub fn parse_and_verify_intent(raw_intent: Value) -> Result<VerifiedIntent, String> {
    let intent: OrderIntent =
        serde_json::from_value(raw_intent.clone()).map_err(|error| format!("bad_intent_json: {error}"))?;
    if !is_valid_identifier(&intent.request_id) || !is_valid_identifier(&intent.key_ref) {
        return Err("bad_intent_json: request_id or key_ref format".to_string());
    }
    if !is_lowercase_hex_of_length(&intent.delegate_key, 64)
        || !is_lowercase_hex_of_length(&intent.delegate_signature, 128)
    {
        return Err("bad_intent_json: delegate_key or delegate_signature format".to_string());
    }
    if !intent.vault_address.is_empty()
        && !(intent.vault_address.starts_with("0x") && is_lowercase_hex_of_length(&intent.vault_address[2..], 40))
    {
        return Err("bad_vault_address".to_string());
    }
    refuse_fields_of_other_kinds(&intent)?;
    let canonical_bytes = verify_delegate_signature(&raw_intent, &intent)?;
    Ok(VerifiedIntent { intent_hash: sha256_hex(&canonical_bytes), intent })
}

pub fn parse_positive_decimal_string(text: &str) -> Option<Decimal> {
    let has_only_plain_digits = {
        let (whole_part, fraction_part) = text.split_once('.').unwrap_or((text, "0"));
        !whole_part.is_empty()
            && !fraction_part.is_empty()
            && whole_part.chars().all(|character| character.is_ascii_digit())
            && fraction_part.chars().all(|character| character.is_ascii_digit())
            && (whole_part == "0" || !whole_part.starts_with('0'))
    };
    if !has_only_plain_digits {
        return None;
    }
    let parsed = Decimal::from_str(text).ok()?;
    if parsed.is_sign_positive() && !parsed.is_zero() {
        Some(parsed)
    } else {
        None
    }
}
