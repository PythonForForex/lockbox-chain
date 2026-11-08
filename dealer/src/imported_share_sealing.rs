use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use hkdf::Hkdf;
use p256::ecdh::{diffie_hellman, EphemeralSecret};
use p256::elliptic_curve::sec1::ToEncodedPoint;
use p256::pkcs8::{DecodePrivateKey, DecodePublicKey};
use p256::{PublicKey, SecretKey};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use x509_cert::der::{DecodePem, Encode};
use x509_cert::Certificate;
use zeroize::Zeroizing;

const KEY_DERIVATION_INFO: &[u8] = b"lockbox-imported-share-sealing-v1";
const NONCE_BYTES: usize = 12;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SealedBox {
    pub ephemeral_public_key: String,
    pub nonce: String,
    pub ciphertext: String,
}

pub fn certificate_der_of_pem(certificate_pem: &str) -> Result<Vec<u8>, String> {
    let certificate = Certificate::from_pem(certificate_pem.as_bytes()).map_err(|error| format!("certificate unreadable: {error}"))?;
    certificate.to_der().map_err(|error| format!("certificate unreadable: {error}"))
}

pub fn certificate_sha256_of_pem(certificate_pem: &str) -> Result<String, String> {
    Ok(hex::encode(Sha256::digest(certificate_der_of_pem(certificate_pem)?)))
}

pub fn public_key_of_certificate_pem(certificate_pem: &str) -> Result<PublicKey, String> {
    let certificate = Certificate::from_pem(certificate_pem.as_bytes()).map_err(|error| format!("certificate unreadable: {error}"))?;
    let subject_public_key_info =
        certificate.tbs_certificate.subject_public_key_info.to_der().map_err(|error| format!("certificate key unreadable: {error}"))?;
    PublicKey::from_public_key_der(&subject_public_key_info).map_err(|_| "certificate key is not a P-256 key".to_string())
}

pub fn private_key_of_pem(private_key_pem: &str) -> Result<SecretKey, String> {
    SecretKey::from_pkcs8_pem(private_key_pem)
        .or_else(|_| SecretKey::from_sec1_pem(private_key_pem))
        .map_err(|_| "private key is not a P-256 key in PKCS#8 or SEC1 PEM".to_string())
}

fn derive_sealing_key(shared_secret: &[u8], ephemeral_public_key: &[u8], recipient_public_key: &[u8]) -> Result<Zeroizing<[u8; 32]>, String> {
    let mut salt = Vec::with_capacity(ephemeral_public_key.len() + recipient_public_key.len());
    salt.extend_from_slice(ephemeral_public_key);
    salt.extend_from_slice(recipient_public_key);
    let mut sealing_key = Zeroizing::new([0u8; 32]);
    Hkdf::<Sha256>::new(Some(&salt), shared_secret)
        .expand(KEY_DERIVATION_INFO, sealing_key.as_mut())
        .map_err(|_| "key derivation failed".to_string())?;
    Ok(sealing_key)
}

pub fn seal_for_recipient(recipient_public_key: &PublicKey, associated_data: &[u8], plaintext: &[u8]) -> Result<SealedBox, String> {
    let ephemeral_secret = EphemeralSecret::random(&mut OsRng);
    let ephemeral_public_key = ephemeral_secret.public_key().to_encoded_point(true);
    let recipient_public_key_bytes = recipient_public_key.to_encoded_point(true);
    let shared_secret = ephemeral_secret.diffie_hellman(recipient_public_key);
    let sealing_key = derive_sealing_key(
        &shared_secret.raw_secret_bytes()[..],
        ephemeral_public_key.as_bytes(),
        recipient_public_key_bytes.as_bytes(),
    )?;
    let mut nonce_bytes = [0u8; NONCE_BYTES];
    OsRng.fill_bytes(&mut nonce_bytes);
    let cipher = Aes256Gcm::new_from_slice(sealing_key.as_ref()).map_err(|_| "cipher key invalid".to_string())?;
    let ciphertext = cipher
        .encrypt(&Nonce::from(nonce_bytes), Payload { msg: plaintext, aad: associated_data })
        .map_err(|_| "encryption failed".to_string())?;
    Ok(SealedBox {
        ephemeral_public_key: hex::encode(ephemeral_public_key.as_bytes()),
        nonce: hex::encode(nonce_bytes),
        ciphertext: hex::encode(ciphertext),
    })
}

pub fn open_with_private_key(own_private_key: &SecretKey, associated_data: &[u8], sealed_box: &SealedBox) -> Result<Zeroizing<Vec<u8>>, String> {
    let ephemeral_public_key_bytes = hex::decode(&sealed_box.ephemeral_public_key).map_err(|_| "ephemeral key is not hex")?;
    let ephemeral_public_key = PublicKey::from_sec1_bytes(&ephemeral_public_key_bytes).map_err(|_| "ephemeral key is not a P-256 point")?;
    let nonce_bytes: [u8; NONCE_BYTES] =
        hex::decode(&sealed_box.nonce).map_err(|_| "nonce is not hex")?.try_into().map_err(|_| "nonce is not 12 bytes")?;
    let ciphertext = hex::decode(&sealed_box.ciphertext).map_err(|_| "ciphertext is not hex")?;
    let own_public_key_bytes = own_private_key.public_key().to_encoded_point(true);
    let shared_secret = diffie_hellman(own_private_key.to_nonzero_scalar(), ephemeral_public_key.as_affine());
    let sealing_key = derive_sealing_key(
        &shared_secret.raw_secret_bytes()[..],
        &ephemeral_public_key_bytes,
        own_public_key_bytes.as_bytes(),
    )?;
    let cipher = Aes256Gcm::new_from_slice(sealing_key.as_ref()).map_err(|_| "cipher key invalid".to_string())?;
    cipher
        .decrypt(&Nonce::from(nonce_bytes), Payload { msg: &ciphertext, aad: associated_data })
        .map(Zeroizing::new)
        .map_err(|_| "share does not open with this signer's key, or it was altered".to_string())
}
