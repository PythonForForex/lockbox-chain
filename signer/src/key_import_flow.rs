use crate::key_generation_flow::{check_ceremony_request, commit_ceremony};
use crate::key_share_store::write_pending_share;
use crate::policy_reader::read_key_ref;
use crate::signer_state::{lookup_key_share, SignerState};
use crate::threshold_protocol::venue_address_of_share;
use lockbox_key_import::imported_share_public_data::{open_sealed_share, ImportedKeyPublicData, SealedShareFile};
use p256::SecretKey;
use serde_json::{json, Value};

pub struct ImportIdentity {
    pub private_key: SecretKey,
    pub certificate_sha256: String,
}

async fn check_key_ref_registered_on_chain(state: &SignerState, public_data: &ImportedKeyPublicData) -> Result<(), String> {
    let key_reference = read_key_ref(&state.node_http_client, &state.config.chain_rest_url, &public_data.key_ref)
        .await
        .map_err(|detail| format!("key_ref_not_registered_on_chain: {detail}"))?;
    let mut chain_committee = key_reference.committee.clone();
    chain_committee.sort();
    if key_reference.venue_address != public_data.venue_address {
        return Err(format!("key_ref_on_chain_has_venue_address_{}", key_reference.venue_address));
    }
    if chain_committee != public_data.committee || key_reference.threshold != public_data.threshold as u32 {
        return Err("key_ref_on_chain_has_another_committee_or_threshold".to_string());
    }
    Ok(())
}

pub async fn handle_import_share(state: &SignerState, identity: &ImportIdentity, body: Value) -> Result<Value, String> {
    let public_data: ImportedKeyPublicData = serde_json::from_value(body["public_data"].clone()).map_err(|error| format!("bad_public_data: {error}"))?;
    let sealed_share: SealedShareFile = serde_json::from_value(body["sealed_share"].clone()).map_err(|error| format!("bad_sealed_share: {error}"))?;
    let own_signer_id = state.config.signer_id.clone();
    if sealed_share.signer_id != own_signer_id {
        return Err(format!("share_is_for_{}_not_{own_signer_id}", sealed_share.signer_id));
    }
    check_ceremony_request(state, &public_data.key_ref, &public_data.committee, public_data.threshold)?;
    check_key_ref_registered_on_chain(state, &public_data).await?;
    let key_share = open_sealed_share(&identity.private_key, &identity.certificate_sha256, &own_signer_id, &public_data, &sealed_share)?;
    write_pending_share(&state.config.state_directory, &public_data.key_ref, &public_data.committee, public_data.threshold, &key_share)?;
    drop(key_share);
    commit_ceremony(state, &public_data.key_ref)?;
    let stored_share = lookup_key_share(state, &public_data.key_ref)?.ok_or("share vanished after commit")?;
    Ok(json!({
        "installed": true,
        "key_ref": public_data.key_ref,
        "signer_id": own_signer_id,
        "venue_address": venue_address_of_share(&stored_share.key_share),
    }))
}
