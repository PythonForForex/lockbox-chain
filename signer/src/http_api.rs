use crate::key_generation_flow::{handle_dkg_abort, handle_dkg_commit, handle_dkg_prepare, handle_dkg_request, handle_dkg_start};
use crate::nonce_record::KeyNonceRecord;
use crate::order_intent::parse_strict_json;
use crate::chain_view::read_or_load_record;
use crate::peer_transport::{open_envelope, seal_envelope, PeerEnvelope};
use crate::signer_state::{deliver_round_message, SignerState};
use crate::peer_signing_handlers::{handle_sign_abort, handle_sign_begin, handle_sign_proposal};
use crate::signing_flow::{handle_sign_request, record_refusal_of_raw};
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::sync::Arc;

const MAXIMUM_PEER_BODY_BYTES: usize = 64 * 1024 * 1024;
const MAXIMUM_CLIENT_BODY_BYTES: usize = 64 * 1024;

async fn handle_health(State(state): State<Arc<SignerState>>) -> Json<Value> {
    let caught_up_key_refs: Vec<String> = state.caught_up_key_refs.lock().expect("caught up lock").iter().cloned().collect();
    Json(json!({
        "status": "ok",
        "signer_id": state.config.signer_id,
        "caught_up_key_refs": caught_up_key_refs,
        "highest_known_height": state.highest_known_height.load(Ordering::SeqCst),
    }))
}

async fn handle_sign(State(state): State<Arc<SignerState>>, body: Bytes) -> Json<Value> {
    let parsed_body = match parse_strict_json(&body) {
        Ok(parsed_body) => parsed_body,
        Err(detail) => {
            let reason = format!("bad_intent_json: {detail}");
            record_refusal_of_raw(&state, &json!({"unparsed": String::from_utf8_lossy(&body)}), &reason);
            return Json(json!({"signed": false, "reason": reason}));
        }
    };
    Json(handle_sign_request(&state, parsed_body["intent"].clone()).await)
}

async fn handle_dkg(State(state): State<Arc<SignerState>>, Json(body): Json<Value>) -> Response {
    match handle_dkg_request(&state, &body).await {
        Ok(reply) => Json(reply).into_response(),
        Err(reason) => (StatusCode::CONFLICT, Json(json!({"reason": reason}))).into_response(),
    }
}

async fn dispatch_peer_message(state: &Arc<SignerState>, sender: &str, kind: &str, body: &Value) -> Result<Value, String> {
    match kind {
        "round_message" => {
            deliver_round_message(
                state,
                body["session_id"].as_str().unwrap_or_default(),
                sender,
                body["sequence"].as_u64().unwrap_or(0),
                body["is_broadcast"].as_bool().unwrap_or(false),
                body["payload"].as_str().unwrap_or_default().to_string(),
            )?;
            Ok(json!({"delivered": true}))
        }
        "sign_proposal" => Ok(handle_sign_proposal(state, sender, body).await),
        "sign_begin" => Ok(handle_sign_begin(state, sender, body)),
        "sign_abort" => Ok(handle_sign_abort(state, sender, body)),
        "record_query" => {
            let record: KeyNonceRecord = read_or_load_record(state, body["key_ref"].as_str().unwrap_or_default()).await?;
            Ok(json!({"record": record, "height": state.highest_known_height.load(Ordering::SeqCst)}))
        }
        "dkg_prepare" => Ok(handle_dkg_prepare(state, sender, body)),
        "dkg_start" => Ok(handle_dkg_start(state, sender, body).await),
        "dkg_commit" => Ok(handle_dkg_commit(state, body)),
        "dkg_abort" => Ok(handle_dkg_abort(state, sender, body)),
        _ => Err(format!("unknown peer message kind {kind}")),
    }
}

async fn handle_peer(State(state): State<Arc<SignerState>>, Json(envelope): Json<PeerEnvelope>) -> Response {
    let body = match open_envelope(&state.directory, &envelope) {
        Ok(body) => body,
        Err(detail) => {
            eprintln!("peer envelope refused: {detail}");
            return (StatusCode::UNAUTHORIZED, detail).into_response();
        }
    };
    match dispatch_peer_message(&state, &envelope.sender, &envelope.kind, &body).await {
        Ok(reply) => Json(seal_envelope(&state.directory, &envelope.sender, &envelope.kind, &reply)).into_response(),
        Err(detail) => (StatusCode::CONFLICT, detail).into_response(),
    }
}

pub fn build_router(state: Arc<SignerState>) -> Router {
    Router::new()
        .route("/health", get(handle_health))
        .route("/sign", post(handle_sign).layer(DefaultBodyLimit::max(MAXIMUM_CLIENT_BODY_BYTES)))
        .route("/dkg", post(handle_dkg).layer(DefaultBodyLimit::max(MAXIMUM_CLIENT_BODY_BYTES)))
        .route("/peer", post(handle_peer).layer(DefaultBodyLimit::max(MAXIMUM_PEER_BODY_BYTES)))
        .with_state(state)
}
