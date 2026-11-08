use crate::hyperliquid_action::keccak256;
use crate::peer_transport::PeerDirectory;
use crate::round_delivery::{build_round_delivery, InboxReceiver, InboxSender, SharedDeliveryBlame};
use cggmp21::key_share::AnyKeyShare;
use cggmp21::keygen::ThresholdMsg;
use cggmp21::key_refresh::AuxOnlyMsg;
use cggmp21::security_level::SecurityLevel128;
use cggmp21::signing::msg::Msg as SigningMsg;
use cggmp21::supported_curves::Secp256k1;
use cggmp21::{DataToSign, ExecutionId, KeyShare, PregeneratedPrimes};
use generic_ec::Scalar;
use k256::ecdsa::{RecoveryId, Signature as K256Signature, VerifyingKey as K256VerifyingKey};
use rand_core::OsRng;
use round_based::MpcParty;
use sha2::{Digest, Sha256};
use std::sync::Arc;

pub type LockboxKeyShare = KeyShare<Secp256k1, SecurityLevel128>;

pub struct RoundChannel {
    pub directory: Arc<PeerDirectory>,
    pub session_id: String,
    pub participants: Vec<String>,
    pub inbox_sender: InboxSender,
    pub inbox_receiver: InboxReceiver,
    pub blame: SharedDeliveryBlame,
}

pub struct EcdsaSignatureWithRecovery {
    pub r: String,
    pub s: String,
    pub v: u8,
}

fn execution_id_bytes(protocol_label: &str, session_id: &str, participants: &[String], binding: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"lockbox-execution-id-v1");
    hasher.update(protocol_label.as_bytes());
    hasher.update((session_id.len() as u64).to_be_bytes());
    hasher.update(session_id.as_bytes());
    for participant in participants {
        hasher.update((participant.len() as u64).to_be_bytes());
        hasher.update(participant.as_bytes());
    }
    hasher.update(binding);
    hasher.finalize().into()
}

pub fn venue_address_of_share(key_share: &LockboxKeyShare) -> String {
    let uncompressed_point = key_share.shared_public_key().to_bytes(false);
    let hash = keccak256(&uncompressed_point[1..]);
    format!("0x{}", hex::encode(&hash[12..]))
}

pub async fn run_distributed_key_generation(
    keygen_channel: RoundChannel,
    auxiliary_channel: RoundChannel,
    own_index: u16,
    committee_size: u16,
    threshold: u16,
) -> Result<LockboxKeyShare, String> {
    let keygen_execution_id = execution_id_bytes(
        "keygen",
        &keygen_channel.session_id,
        &keygen_channel.participants,
        &threshold.to_be_bytes(),
    );
    let keygen_delivery = build_round_delivery::<ThresholdMsg<Secp256k1, SecurityLevel128, Sha256>>(
        keygen_channel.directory,
        keygen_channel.session_id,
        keygen_channel.participants,
        keygen_channel.inbox_sender,
        keygen_channel.inbox_receiver,
        keygen_channel.blame.clone(),
    );
    let keygen_party = MpcParty::connected((keygen_delivery.incoming, keygen_delivery.outgoing));
    let incomplete_share = cggmp21::keygen::<Secp256k1>(ExecutionId::new(&keygen_execution_id), own_index, committee_size)
        .set_threshold(threshold)
        .set_security_level::<SecurityLevel128>()
        .start(&mut OsRng, keygen_party)
        .await
        .map_err(|error| format!("keygen failed: {error:?}"))?;

    let auxiliary_execution_id = execution_id_bytes(
        "auxiliary",
        &auxiliary_channel.session_id,
        &auxiliary_channel.participants,
        &threshold.to_be_bytes(),
    );
    let primes = PregeneratedPrimes::<SecurityLevel128>::generate(&mut OsRng);
    let auxiliary_delivery = build_round_delivery::<AuxOnlyMsg<Sha256, SecurityLevel128>>(
        auxiliary_channel.directory,
        auxiliary_channel.session_id,
        auxiliary_channel.participants,
        auxiliary_channel.inbox_sender,
        auxiliary_channel.inbox_receiver,
        auxiliary_channel.blame.clone(),
    );
    let auxiliary_party = MpcParty::connected((auxiliary_delivery.incoming, auxiliary_delivery.outgoing));
    let auxiliary_info = cggmp21::aux_info_gen(
        ExecutionId::new(&auxiliary_execution_id),
        own_index,
        committee_size,
        primes,
    )
    .start(&mut OsRng, auxiliary_party)
    .await
    .map_err(|error| format!("auxiliary generation failed: {error:?}"))?;

    LockboxKeyShare::from_parts((incomplete_share, auxiliary_info)).map_err(|error| format!("key share invalid: {error:?}"))
}

// Only `signing().sign()` is called: the full interactive protocol. `generate_presignature` must never appear in
// this crate (CVE-2025-66017; cggmp21 0.6.3 keeps that API open and signs any scalar).
pub async fn run_interactive_signing(
    channel: RoundChannel,
    own_position: u16,
    keygen_indices_of_participants: &[u16],
    key_share: &LockboxKeyShare,
    digest: &[u8; 32],
) -> Result<EcdsaSignatureWithRecovery, String> {
    let execution_id = execution_id_bytes("signing", &channel.session_id, &channel.participants, digest);
    let delivery = build_round_delivery::<SigningMsg<Secp256k1, Sha256>>(
        channel.directory,
        channel.session_id,
        channel.participants,
        channel.inbox_sender,
        channel.inbox_receiver,
        channel.blame.clone(),
    );
    let party = MpcParty::connected((delivery.incoming, delivery.outgoing));
    let data_to_sign = DataToSign::from_scalar(Scalar::<Secp256k1>::from_be_bytes_mod_order(digest));
    let signature = cggmp21::signing(ExecutionId::new(&execution_id), own_position, keygen_indices_of_participants, key_share)
        .sign(&mut OsRng, party, data_to_sign)
        .await
        .map_err(|error| format!("signing failed: {error:?}"))?
        .normalize_s();
    signature
        .verify(&key_share.shared_public_key(), &data_to_sign)
        .map_err(|_| "signature does not verify against the key".to_string())?;
    let mut signature_bytes = vec![0u8; cggmp21::signing::Signature::<Secp256k1>::serialized_len()];
    signature.write_to_slice(&mut signature_bytes);
    attach_recovery_id(&signature_bytes, digest, &key_share.shared_public_key().to_bytes(true))
}

fn attach_recovery_id(
    signature_bytes: &[u8],
    digest: &[u8; 32],
    compressed_public_key: &[u8],
) -> Result<EcdsaSignatureWithRecovery, String> {
    let parsed_signature = K256Signature::from_slice(signature_bytes)
        .map_err(|error| format!("r, s are not valid scalars: {error}"))?;
    if parsed_signature.normalize_s().is_some() {
        return Err("signature is not low-s".to_string());
    }
    let verifying_key = K256VerifyingKey::from_sec1_bytes(compressed_public_key)
        .map_err(|error| format!("public key invalid: {error}"))?;
    let recovery_id = RecoveryId::trial_recovery_from_prehash(&verifying_key, digest, &parsed_signature)
        .map_err(|error| format!("recovery id not found: {error}"))?;
    let (r_bytes, s_bytes) = signature_bytes.split_at(32);
    Ok(EcdsaSignatureWithRecovery {
        r: hex::encode(r_bytes),
        s: hex::encode(s_bytes),
        v: 27 + recovery_id.to_byte(),
    })
}
