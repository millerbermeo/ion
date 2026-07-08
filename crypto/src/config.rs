use std::sync::Arc;

use rustls::crypto::ring::default_provider;
use rustls::{ClientConfig, ServerConfig};

use crate::error::CryptoError;
use crate::identity::Identity;
use crate::policy::PairingMode;
use crate::trust_store::TrustStore;
use crate::verifier::{TofuClientVerifier, TofuServerVerifier};

/// Construye la configuración TLS del lado cliente: TLS 1.3 únicamente,
/// verificación de servidor TOFU y autenticación mutua (el cliente también
/// presenta su propio certificado).
///
/// # Errors
///
/// Devuelve [`CryptoError::Tls`] si `rustls` rechaza la configuración (por
/// ejemplo, una clave privada con un formato no soportado).
pub fn client_config(
    identity: &Identity,
    trust_store: Arc<dyn TrustStore>,
    mode: PairingMode,
) -> Result<Arc<ClientConfig>, CryptoError> {
    let provider = Arc::new(default_provider());
    let verifier = Arc::new(TofuServerVerifier::new(
        Arc::clone(&provider),
        trust_store,
        mode,
    ));

    let config = ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(CryptoError::Tls)?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_client_auth_cert(identity.cert_chain(), identity.key())
        .map_err(CryptoError::Tls)?;

    Ok(Arc::new(config))
}

/// Construye la configuración TLS del lado servidor: TLS 1.3 únicamente y
/// verificación de cliente TOFU obligatoria (sin certificado de cliente no
/// hay conexión — "nunca asumir que el cliente es confiable").
///
/// # Errors
///
/// Devuelve [`CryptoError::Tls`] si `rustls` rechaza la configuración.
pub fn server_config(
    identity: &Identity,
    trust_store: Arc<dyn TrustStore>,
    mode: PairingMode,
) -> Result<Arc<ServerConfig>, CryptoError> {
    let provider = Arc::new(default_provider());
    let verifier = Arc::new(TofuClientVerifier::new(
        Arc::clone(&provider),
        trust_store,
        mode,
    ));

    let config = ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(CryptoError::Tls)?
        .with_client_cert_verifier(verifier)
        .with_single_cert(identity.cert_chain(), identity.key())
        .map_err(CryptoError::Tls)?;

    Ok(Arc::new(config))
}
