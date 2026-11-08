use crate::imported_share_sealing::{open_with_private_key, SealedBox};
use cggmp21::key_share::AnyKeyShare;
use cggmp21::security_level::SecurityLevel128;
use cggmp21::supported_curves::Secp256k1;
use cggmp21::KeyShare;
use generic_ec::Point;
use p256::SecretKey;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sha3::Keccak256;
use std::collections::BTreeMap;

pub const PUBLIC_DATA_FORMAT: &str = "lockbox-imported-key-public-data-v1";
pub const SEALED_SHARE_FORMAT: &str = "lockbox-imported-key-sealed-share-v1";
const ASSOCIATED_DATA_DOMAIN: &[u8] = b"lockbox-imported-share-associated-data-v1";

pub type ImportedKeyShare = KeyShare<Secp256k1, SecurityLevel128>;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ImportedKeyPublicData {
    pub format: String,
    pub key_ref: String,
    pub committee: Vec<String>,
    pub threshold: u16,
    pub venue_address: String,
    pub shared_public_key: String,
    pub public_shares: Vec<String>,
    pub recipient_certificate_sha256: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SealedShareFile {
    pub format: String,
    pub key_ref: String,
    pub signer_id: String,
    pub recipient_certificate_sha256: String,
    pub public_data_sha256: String,
    pub sealed: SealedBox,
}

pub fn venue_address_of_point(point: &Point<Secp256k1>) -> String {
    let uncompressed_point = point.to_bytes(false);
    let hash = Keccak256::digest(&uncompressed_point[1..]);
    format!("0x{}", hex::encode(&hash[12..]))
}

pub fn public_data_sha256(public_data: &ImportedKeyPublicData) -> String {
    hex::encode(Sha256::digest(serde_json::to_vec(public_data).expect("public data serializes")))
}

pub fn sealing_associated_data(key_ref: &str, signer_id: &str, recipient_certificate_sha256: &str, public_data_digest: &str) -> Vec<u8> {
    let mut associated_data = ASSOCIATED_DATA_DOMAIN.to_vec();
    for field in [SEALED_SHARE_FORMAT, key_ref, signer_id, recipient_certificate_sha256, public_data_digest] {
        associated_data.extend_from_slice(&(field.len() as u64).to_be_bytes());
        associated_data.extend_from_slice(field.as_bytes());
    }
    associated_data
}

pub fn check_committee_shape(committee: &[String], threshold: u16) -> Result<(), String> {
    if committee.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err("committee_not_sorted_or_has_duplicates".to_string());
    }
    if threshold < 2 || threshold as usize > committee.len() {
        return Err("threshold_out_of_range".to_string());
    }
    Ok(())
}

pub fn public_data_of_share(
    key_ref: &str,
    committee: &[String],
    recipient_certificate_sha256: BTreeMap<String, String>,
    key_share: &ImportedKeyShare,
) -> ImportedKeyPublicData {
    let shared_public_key = key_share.shared_public_key();
    ImportedKeyPublicData {
        format: PUBLIC_DATA_FORMAT.to_string(),
        key_ref: key_ref.to_string(),
        committee: committee.to_vec(),
        threshold: key_share.min_signers(),
        venue_address: venue_address_of_point(&shared_public_key),
        shared_public_key: hex::encode(shared_public_key.to_bytes(true)),
        public_shares: key_share.core.key_info.public_shares.iter().map(|share| hex::encode(share.to_bytes(true))).collect(),
        recipient_certificate_sha256,
    }
}

pub fn check_share_against_public_data(key_share: &ImportedKeyShare, public_data: &ImportedKeyPublicData, own_signer_id: &str) -> Result<(), String> {
    check_committee_shape(&public_data.committee, public_data.threshold)?;
    let own_position = public_data.committee.iter().position(|signer| signer == own_signer_id).ok_or("this_signer_not_in_committee")?;
    if key_share.core.i as usize != own_position {
        return Err(format!("share_index_{}_is_not_this_signers_position_{own_position}", key_share.core.i));
    }
    if key_share.n() as usize != public_data.committee.len() || key_share.min_signers() != public_data.threshold {
        return Err("share_committee_size_or_threshold_differs_from_public_data".to_string());
    }
    let recomputed = public_data_of_share(&public_data.key_ref, &public_data.committee, public_data.recipient_certificate_sha256.clone(), key_share);
    if recomputed != *public_data {
        return Err("share_public_keys_differ_from_public_data".to_string());
    }
    Ok(())
}

pub fn open_sealed_share(
    own_private_key: &SecretKey,
    own_certificate_sha256: &str,
    own_signer_id: &str,
    public_data: &ImportedKeyPublicData,
    sealed_share: &SealedShareFile,
) -> Result<ImportedKeyShare, String> {
    if public_data.format != PUBLIC_DATA_FORMAT || sealed_share.format != SEALED_SHARE_FORMAT {
        return Err("unknown_import_format".to_string());
    }
    if sealed_share.signer_id != own_signer_id {
        return Err(format!("share_is_for_{}_not_{own_signer_id}", sealed_share.signer_id));
    }
    if sealed_share.key_ref != public_data.key_ref {
        return Err("share_key_ref_differs_from_public_data".to_string());
    }
    let digest = public_data_sha256(public_data);
    if sealed_share.public_data_sha256 != digest {
        return Err("share_was_made_with_other_public_data".to_string());
    }
    if sealed_share.recipient_certificate_sha256 != own_certificate_sha256
        || public_data.recipient_certificate_sha256.get(own_signer_id).map(String::as_str) != Some(own_certificate_sha256)
    {
        return Err("share_sealed_to_another_certificate".to_string());
    }
    let associated_data = sealing_associated_data(&sealed_share.key_ref, own_signer_id, own_certificate_sha256, &digest);
    let plaintext = open_with_private_key(own_private_key, &associated_data, &sealed_share.sealed)?;
    let key_share: ImportedKeyShare = serde_json::from_slice(&plaintext).map_err(|error| format!("share_invalid: {error}"))?;
    check_share_against_public_data(&key_share, public_data, own_signer_id)?;
    Ok(key_share)
}
