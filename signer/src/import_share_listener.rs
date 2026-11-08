use crate::key_import_flow::{handle_import_share, ImportIdentity};
use crate::signer_config::ImportShareListenerConfig;
use crate::signer_state::SignerState;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder as ConnectionBuilder;
use hyper_util::service::TowerToHyperService;
use lockbox_key_import::imported_share_sealing::{certificate_der_of_pem, certificate_sha256_of_pem, private_key_of_pem};
use rustls::client::danger::HandshakeSignatureValid;
use rustls::crypto::{ring, verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{CertificateError, DigitallySignedStruct, DistinguishedName, Error as TlsError, ServerConfig, SignatureScheme};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tokio_rustls::TlsAcceptor;

const MAXIMUM_IMPORT_BODY_BYTES: usize = 1024 * 1024;
const HANDSHAKE_TIMEOUT_SECONDS: u64 = 10;

#[derive(Debug)]
struct PinnedClientCertificateVerifier {
    pinned_certificate_der: Vec<u8>,
    provider: Arc<CryptoProvider>,
}

// Only the one pinned certificate is accepted, byte for byte: there is no CA, so chain and name checks do not apply.
impl ClientCertVerifier for PinnedClientCertificateVerifier {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(&self, end_entity: &CertificateDer<'_>, intermediates: &[CertificateDer<'_>], _now: UnixTime) -> Result<ClientCertVerified, TlsError> {
        if intermediates.is_empty() && end_entity.as_ref() == self.pinned_certificate_der.as_slice() {
            return Ok(ClientCertVerified::assertion());
        }
        Err(TlsError::InvalidCertificate(CertificateError::ApplicationVerificationFailure))
    }

    fn verify_tls12_signature(&self, message: &[u8], certificate: &CertificateDer<'_>, signature: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, TlsError> {
        verify_tls12_signature(message, certificate, signature, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(&self, message: &[u8], certificate: &CertificateDer<'_>, signature: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, TlsError> {
        verify_tls13_signature(message, certificate, signature, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

struct ImportListenerState {
    signer_state: Arc<SignerState>,
    identity: ImportIdentity,
}

fn read_text(path: &std::path::Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|error| format!("read {}: {error}", path.display()))
}

fn build_tls_server_config(config: &ImportShareListenerConfig) -> Result<ServerConfig, String> {
    let provider = Arc::new(ring::default_provider());
    let pinned_certificate_der = certificate_der_of_pem(&read_text(&config.pinned_client_certificate_file)?)?;
    let server_certificate = CertificateDer::from_pem_file(&config.certificate_file).map_err(|error| format!("server certificate: {error}"))?;
    let server_private_key = PrivateKeyDer::from_pem_file(&config.private_key_file).map_err(|error| format!("server private key: {error}"))?;
    let verifier = Arc::new(PinnedClientCertificateVerifier { pinned_certificate_der, provider: provider.clone() });
    ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|error| error.to_string())?
        .with_client_cert_verifier(verifier)
        .with_single_cert(vec![server_certificate], server_private_key)
        .map_err(|error| format!("import listener TLS setup: {error}"))
}

async fn handle_import_share_request(State(listener_state): State<Arc<ImportListenerState>>, Json(body): Json<Value>) -> Response {
    match handle_import_share(&listener_state.signer_state, &listener_state.identity, body).await {
        Ok(reply) => Json(reply).into_response(),
        Err(reason) => {
            eprintln!("import-share refused: {reason}");
            (StatusCode::CONFLICT, Json(json!({"installed": false, "reason": reason}))).into_response()
        }
    }
}

pub async fn start_import_share_listener(signer_state: Arc<SignerState>, config: ImportShareListenerConfig) -> Result<(), String> {
    let tls_acceptor = TlsAcceptor::from(Arc::new(build_tls_server_config(&config)?));
    let identity = ImportIdentity {
        private_key: private_key_of_pem(&read_text(&config.private_key_file)?)?,
        certificate_sha256: certificate_sha256_of_pem(&read_text(&config.certificate_file)?)?,
    };
    let router = Router::new()
        .route("/import-share", post(handle_import_share_request).layer(DefaultBodyLimit::max(MAXIMUM_IMPORT_BODY_BYTES)))
        .with_state(Arc::new(ImportListenerState { signer_state, identity }));
    let listener = tokio::net::TcpListener::bind(&config.listen_address).await.map_err(|error| format!("import listener bind: {error}"))?;
    eprintln!("import-share listener on {} (mutual TLS, pinned client certificate)", config.listen_address);
    tokio::spawn(async move {
        loop {
            let Ok((connection, peer_address)) = listener.accept().await else { continue };
            let tls_acceptor = tls_acceptor.clone();
            let router = router.clone();
            tokio::spawn(async move {
                let handshake = tokio::time::timeout(Duration::from_secs(HANDSHAKE_TIMEOUT_SECONDS), tls_acceptor.accept(connection)).await;
                let tls_stream = match handshake {
                    Ok(Ok(tls_stream)) => tls_stream,
                    Ok(Err(error)) => return eprintln!("import-share TLS refused from {peer_address}: {error}"),
                    Err(_) => return eprintln!("import-share TLS handshake timed out from {peer_address}"),
                };
                let service = TowerToHyperService::new(router);
                let _ = ConnectionBuilder::new(TokioExecutor::new()).serve_connection(TokioIo::new(tls_stream), service).await;
            });
        }
    });
    Ok(())
}
