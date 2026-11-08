use crate::decision_recorder::DecisionEntry;
use crate::signer_config::SignerConfig;
use k256::ecdsa::signature::Signer;
use k256::ecdsa::{Signature, SigningKey};
use sha2::{Digest, Sha256};

const RECORD_DECISION_TYPE_URL: &str = "/lockbox.v1.MsgRecordDecision";
const PUBLIC_KEY_TYPE_URL: &str = "/cosmos.crypto.secp256k1.PubKey";
const SIGN_MODE_DIRECT: u64 = 1;
const RECORD_DECISION_GAS_LIMIT: u64 = 400_000;

pub struct ChainSigningIdentity {
    signing_key: SigningKey,
    compressed_public_key: Vec<u8>,
    pub account_address: String,
    chain_id: String,
}

pub fn load_chain_signing_identity(config: &SignerConfig) -> Result<ChainSigningIdentity, String> {
    let raw_key = std::fs::read_to_string(&config.chain_key_file).map_err(|error| format!("read chain key: {error}"))?;
    let key_bytes = hex::decode(raw_key.trim()).map_err(|_| "chain key is not hex")?;
    let signing_key = SigningKey::from_slice(&key_bytes).map_err(|_| "chain key is not a secp256k1 key")?;
    let compressed_public_key = signing_key.verifying_key().to_encoded_point(true).as_bytes().to_vec();
    Ok(ChainSigningIdentity {
        signing_key,
        compressed_public_key,
        account_address: config.chain_account_address.clone(),
        chain_id: config.chain_id.clone(),
    })
}

fn write_varint(mut value: u64, output: &mut Vec<u8>) {
    while value >= 0x80 {
        output.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    output.push(value as u8);
}

// Proto3 leaves out a field holding its default. Go's encoder does the same, and SIGN_MODE_DIRECT signs these bytes.
fn write_bytes_field(field_number: u64, value: &[u8], output: &mut Vec<u8>) {
    if value.is_empty() {
        return;
    }
    write_varint(field_number << 3 | 2, output);
    write_varint(value.len() as u64, output);
    output.extend_from_slice(value);
}

fn write_varint_field(field_number: u64, value: u64, output: &mut Vec<u8>) {
    if value == 0 {
        return;
    }
    write_varint(field_number << 3, output);
    write_varint(value, output);
}

fn encode_any(type_url: &str, value: &[u8]) -> Vec<u8> {
    let mut any = Vec::new();
    write_bytes_field(1, type_url.as_bytes(), &mut any);
    write_bytes_field(2, value, &mut any);
    any
}

fn encode_record_decision_message(identity: &ChainSigningIdentity, entry: &DecisionEntry) -> Vec<u8> {
    let mut message = Vec::new();
    write_bytes_field(1, identity.account_address.as_bytes(), &mut message);
    write_bytes_field(2, entry.request_id.as_bytes(), &mut message);
    write_bytes_field(3, entry.key_ref.as_bytes(), &mut message);
    write_bytes_field(4, entry.signer_id.as_bytes(), &mut message);
    write_bytes_field(5, entry.decision.as_bytes(), &mut message);
    write_bytes_field(6, entry.reason.as_bytes(), &mut message);
    write_bytes_field(7, entry.intent_hash.as_bytes(), &mut message);
    message
}

fn encode_transaction_body(message_any: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    write_bytes_field(1, message_any, &mut body);
    body
}

fn encode_auth_info(identity: &ChainSigningIdentity, sequence: u64) -> Vec<u8> {
    let mut public_key = Vec::new();
    write_bytes_field(1, &identity.compressed_public_key, &mut public_key);
    let mut single_mode = Vec::new();
    write_varint_field(1, SIGN_MODE_DIRECT, &mut single_mode);
    let mut mode_info = Vec::new();
    write_bytes_field(1, &single_mode, &mut mode_info);
    let mut signer_info = Vec::new();
    write_bytes_field(1, &encode_any(PUBLIC_KEY_TYPE_URL, &public_key), &mut signer_info);
    write_bytes_field(2, &mode_info, &mut signer_info);
    write_varint_field(3, sequence, &mut signer_info);
    let mut fee = Vec::new();
    write_varint_field(2, RECORD_DECISION_GAS_LIMIT, &mut fee);
    let mut auth_info = Vec::new();
    write_bytes_field(1, &signer_info, &mut auth_info);
    write_bytes_field(2, &fee, &mut auth_info);
    auth_info
}

pub fn build_record_decision_transaction(identity: &ChainSigningIdentity, entry: &DecisionEntry, account_number: u64, sequence: u64) -> Vec<u8> {
    let message_any = encode_any(RECORD_DECISION_TYPE_URL, &encode_record_decision_message(identity, entry));
    let body_bytes = encode_transaction_body(&message_any);
    let auth_info_bytes = encode_auth_info(identity, sequence);
    let mut sign_document = Vec::new();
    write_bytes_field(1, &body_bytes, &mut sign_document);
    write_bytes_field(2, &auth_info_bytes, &mut sign_document);
    write_bytes_field(3, identity.chain_id.as_bytes(), &mut sign_document);
    write_varint_field(4, account_number, &mut sign_document);
    let signature: Signature = identity.signing_key.sign(&sign_document);
    let signature = signature.normalize_s().unwrap_or(signature);
    let mut transaction = Vec::new();
    write_bytes_field(1, &body_bytes, &mut transaction);
    write_bytes_field(2, &auth_info_bytes, &mut transaction);
    write_bytes_field(3, &signature.to_bytes(), &mut transaction);
    transaction
}

pub fn transaction_hash_hex(transaction_bytes: &[u8]) -> String {
    hex::encode_upper(Sha256::digest(transaction_bytes))
}
