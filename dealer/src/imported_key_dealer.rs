use crate::imported_share_public_data::{
    check_committee_shape, public_data_of_share, public_data_sha256, sealing_associated_data, venue_address_of_point, ImportedKeyPublicData,
    ImportedKeyShare, SealedShareFile, SEALED_SHARE_FORMAT,
};
use crate::imported_share_sealing::{certificate_sha256_of_pem, public_key_of_certificate_pem, seal_for_recipient};
use cggmp21::security_level::SecurityLevel128;
use cggmp21::supported_curves::Secp256k1;
use cggmp21::PregeneratedPrimes;
use generic_ec::{NonZero, Point, SecretScalar};
use rand_core::OsRng;
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use zeroize::Zeroizing;

const OWNER_READ_WRITE_ONLY: u32 = 0o600;
const OWNER_ONLY_DIRECTORY: u32 = 0o700;
// Sized so serde_json never grows the buffer: a reallocation would free a copy of the share that is not wiped.
const SHARE_PLAINTEXT_CAPACITY_BYTES: usize = 1024 * 1024;

pub struct ImportRecipient {
    pub signer_id: String,
    pub certificate_pem: String,
}

pub struct DealerOutput {
    pub public_data: ImportedKeyPublicData,
    pub sealed_shares: Vec<SealedShareFile>,
}

pub fn parse_secret_key_hex(secret_text: Zeroizing<String>) -> Result<NonZero<SecretScalar<Secp256k1>>, String> {
    let trimmed = secret_text.trim();
    let without_prefix = trimmed.strip_prefix("0x").unwrap_or(trimmed);
    if without_prefix.len() != 64 {
        return Err("secret key must be 32 bytes as 64 hex characters".to_string());
    }
    let mut secret_bytes = Zeroizing::new([0u8; 32]);
    hex::decode_to_slice(without_prefix, secret_bytes.as_mut()).map_err(|_| "secret key is not hex".to_string())?;
    let scalar = SecretScalar::<Secp256k1>::from_be_bytes(secret_bytes.as_ref()).map_err(|_| "secret key is not below the curve order".to_string())?;
    NonZero::from_secret_scalar(scalar).ok_or("secret key is zero".to_string())
}

pub fn venue_address_of_secret_key(secret_key: &NonZero<SecretScalar<Secp256k1>>) -> String {
    venue_address_of_point(&(Point::<Secp256k1>::generator() * secret_key))
}

fn generate_paillier_primes_in_parallel(committee_size: usize) -> Vec<PregeneratedPrimes<SecurityLevel128>> {
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..committee_size).map(|_| scope.spawn(|| PregeneratedPrimes::<SecurityLevel128>::generate(&mut OsRng))).collect();
        workers.into_iter().map(|worker| worker.join().expect("prime generation thread")).collect()
    })
}

fn serialize_share_into_wiped_buffer(key_share: &ImportedKeyShare) -> Result<Zeroizing<Vec<u8>>, String> {
    let mut plaintext = Zeroizing::new(Vec::with_capacity(SHARE_PLAINTEXT_CAPACITY_BYTES));
    serde_json::to_writer(&mut *plaintext, key_share).map_err(|error| format!("share does not serialize: {error}"))?;
    if plaintext.capacity() != SHARE_PLAINTEXT_CAPACITY_BYTES {
        return Err("share outgrew its wiped buffer".to_string());
    }
    Ok(plaintext)
}

pub fn deal_imported_key(
    secret_key: NonZero<SecretScalar<Secp256k1>>,
    expected_venue_address: &str,
    key_ref: &str,
    recipients: &[ImportRecipient],
    threshold: u16,
) -> Result<DealerOutput, String> {
    if key_ref.is_empty() {
        return Err("key_ref_empty".to_string());
    }
    let derived_venue_address = venue_address_of_secret_key(&secret_key);
    if derived_venue_address != expected_venue_address.to_lowercase() {
        return Err(format!("derived address {derived_venue_address} is not the expected {expected_venue_address}; nothing was split"));
    }
    let committee: Vec<String> = recipients.iter().map(|recipient| recipient.signer_id.clone()).collect();
    check_committee_shape(&committee, threshold)?;
    let mut recipient_public_keys = Vec::with_capacity(recipients.len());
    let mut recipient_certificate_sha256 = BTreeMap::new();
    for recipient in recipients {
        recipient_public_keys.push(public_key_of_certificate_pem(&recipient.certificate_pem)?);
        recipient_certificate_sha256.insert(recipient.signer_id.clone(), certificate_sha256_of_pem(&recipient.certificate_pem)?);
    }
    let primes = generate_paillier_primes_in_parallel(recipients.len()).into_iter().map(PregeneratedPrimes::split).collect();
    let key_shares = cggmp21::trusted_dealer::builder::<Secp256k1, SecurityLevel128>(recipients.len() as u16)
        .set_threshold(Some(threshold))
        .set_shared_secret_key(secret_key)
        .set_pregenerated_primes(primes)
        .generate_shares(&mut OsRng)
        .map_err(|error| format!("trusted dealer failed: {error}"))?;
    let public_data = public_data_of_share(key_ref, &committee, recipient_certificate_sha256, &key_shares[0]);
    if public_data.venue_address != derived_venue_address {
        return Err("shares do not carry the imported key".to_string());
    }
    let digest = public_data_sha256(&public_data);
    let mut sealed_shares = Vec::with_capacity(recipients.len());
    for ((recipient, recipient_public_key), key_share) in recipients.iter().zip(&recipient_public_keys).zip(&key_shares) {
        let certificate_digest = &public_data.recipient_certificate_sha256[&recipient.signer_id];
        let associated_data = sealing_associated_data(key_ref, &recipient.signer_id, certificate_digest, &digest);
        let plaintext = serialize_share_into_wiped_buffer(key_share)?;
        sealed_shares.push(SealedShareFile {
            format: SEALED_SHARE_FORMAT.to_string(),
            key_ref: key_ref.to_string(),
            signer_id: recipient.signer_id.clone(),
            recipient_certificate_sha256: certificate_digest.clone(),
            public_data_sha256: digest.clone(),
            sealed: seal_for_recipient(recipient_public_key, &associated_data, &plaintext)?,
        });
    }
    drop(key_shares);
    Ok(DealerOutput { public_data, sealed_shares })
}

fn write_owner_only_file(path: &Path, contents: &[u8]) -> Result<(), String> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(OWNER_READ_WRITE_ONLY)
        .open(path)
        .map_err(|error| format!("create {}: {error}", path.display()))?;
    file.write_all(contents).and_then(|_| file.sync_all()).map_err(|error| format!("write {}: {error}", path.display()))
}

pub fn write_dealer_output(output_directory: &Path, dealer_output: &DealerOutput) -> Result<(), String> {
    if output_directory.exists() && fs::read_dir(output_directory).map_err(|error| error.to_string())?.next().is_some() {
        return Err(format!("output directory {} is not empty", output_directory.display()));
    }
    fs::create_dir_all(output_directory).map_err(|error| error.to_string())?;
    fs::set_permissions(output_directory, fs::Permissions::from_mode(OWNER_ONLY_DIRECTORY)).map_err(|error| error.to_string())?;
    let public_data_text = serde_json::to_string_pretty(&dealer_output.public_data).map_err(|error| error.to_string())?;
    write_owner_only_file(&output_directory.join("public_data.json"), public_data_text.as_bytes())?;
    for sealed_share in &dealer_output.sealed_shares {
        let sealed_share_text = serde_json::to_string_pretty(sealed_share).map_err(|error| error.to_string())?;
        write_owner_only_file(&output_directory.join(format!("share-{}.json", sealed_share.signer_id)), sealed_share_text.as_bytes())?;
    }
    Ok(())
}
