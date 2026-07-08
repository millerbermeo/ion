use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{DigitallySignedStruct, DistinguishedName, Error as TlsError, SignatureScheme};
use rustls_pki_types::{CertificateDer, ServerName, UnixTime};

use crate::fingerprint::Fingerprint;
use crate::policy::PairingMode;
use crate::trust_store::TrustStore;

/// Decide si `end_entity` debe aceptarse, según el `TrustStore` y la
/// [`PairingMode`] activa. Compartida por el verificador de cliente y el de
/// servidor: la lógica de confianza es idéntica en ambos lados, solo cambia
/// qué trait de rustls la invoca.
fn evaluate_trust(
    trust_store: &dyn TrustStore,
    mode: PairingMode,
    end_entity: &CertificateDer<'_>,
) -> Result<(), TlsError> {
    let fingerprint = Fingerprint::of_der(end_entity);
    if trust_store.is_trusted(&fingerprint) {
        return Ok(());
    }
    match mode {
        PairingMode::AutoTrustOnFirstUse => {
            tracing::info!(%fingerprint, "confiando en certificado nuevo (TOFU)");
            trust_store.trust(fingerprint);
            Ok(())
        }
        PairingMode::RejectUnknown => {
            tracing::warn!(%fingerprint, "certificado desconocido rechazado (posible MITM)");
            Err(TlsError::General(format!(
                "fingerprint no confiable: {fingerprint}"
            )))
        }
    }
}

/// Verificador de certificado de servidor (lado cliente de la conexión TLS)
/// con confianza TOFU en vez de una cadena de CA.
#[derive(Debug)]
pub struct TofuServerVerifier {
    provider: Arc<CryptoProvider>,
    trust_store: Arc<dyn TrustStore>,
    mode: PairingMode,
}

impl TofuServerVerifier {
    #[must_use]
    pub fn new(
        provider: Arc<CryptoProvider>,
        trust_store: Arc<dyn TrustStore>,
        mode: PairingMode,
    ) -> Self {
        Self {
            provider,
            trust_store,
            mode,
        }
    }
}

impl ServerCertVerifier for TofuServerVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        evaluate_trust(self.trust_store.as_ref(), self.mode, end_entity)?;
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Verificador de certificado de cliente (lado servidor de la conexión TLS)
/// con la misma política TOFU. `IonConnect` usa TLS mutuo: cada lado
/// autentica al otro, no solo el cliente al servidor.
#[derive(Debug)]
pub struct TofuClientVerifier {
    provider: Arc<CryptoProvider>,
    trust_store: Arc<dyn TrustStore>,
    mode: PairingMode,
}

impl TofuClientVerifier {
    #[must_use]
    pub fn new(
        provider: Arc<CryptoProvider>,
        trust_store: Arc<dyn TrustStore>,
        mode: PairingMode,
    ) -> Self {
        Self {
            provider,
            trust_store,
            mode,
        }
    }
}

impl ClientCertVerifier for TofuClientVerifier {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, TlsError> {
        evaluate_trust(self.trust_store.as_ref(), self.mode, end_entity)?;
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}
