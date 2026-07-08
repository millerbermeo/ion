use rcgen::CertifiedKey;
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};

use crate::error::CryptoError;
use crate::fingerprint::Fingerprint;

/// Certificado autofirmado + clave privada de un equipo `IonConnect`.
///
/// Se genera una vez por instalación (`Identity::generate`) para que el
/// `DeviceId` y el fingerprint TOFU sean estables entre reinicios. Este
/// crate no decide dónde ni cómo se persiste en disco (eso es del futuro
/// crate `config`) — solo expone el PEM ya codificado vía
/// [`Identity::cert_pem`]/[`Identity::key_pem`] para que el llamador lo
/// escriba donde le corresponda, y [`Identity::from_pem`] para recargarlo.
pub struct Identity {
    cert: CertificateDer<'static>,
    key: PrivateKeyDer<'static>,
    cert_pem: String,
    key_pem: String,
}

impl Clone for Identity {
    fn clone(&self) -> Self {
        Self {
            cert: self.cert.clone(),
            key: self.key.clone_key(),
            cert_pem: self.cert_pem.clone(),
            key_pem: self.key_pem.clone(),
        }
    }
}

impl Identity {
    /// Genera una identidad nueva con un certificado autofirmado.
    ///
    /// # Errors
    ///
    /// Devuelve [`CryptoError::CertificateGeneration`] si `rcgen` falla al
    /// generar el par de claves o el certificado.
    pub fn generate() -> Result<Self, CryptoError> {
        let CertifiedKey { cert, signing_key } =
            rcgen::generate_simple_self_signed(["ionconnect.local".to_string()])?;
        let cert_pem = cert.pem();
        let key_pem = signing_key.serialize_pem();
        Ok(Self {
            cert: cert.der().clone(),
            key: PrivateKeyDer::from(signing_key),
            cert_pem,
            key_pem,
        })
    }

    /// Reconstruye una identidad previamente persistida en formato PEM.
    ///
    /// # Errors
    ///
    /// Devuelve [`CryptoError::Pem`] si `cert_pem` o `key_pem` no contienen
    /// una sección PEM válida del tipo esperado.
    pub fn from_pem(cert_pem: &str, key_pem: &str) -> Result<Self, CryptoError> {
        let cert = CertificateDer::from_pem_slice(cert_pem.as_bytes())?;
        let key = PrivateKeyDer::from_pem_slice(key_pem.as_bytes())?;
        Ok(Self {
            cert,
            key,
            cert_pem: cert_pem.to_string(),
            key_pem: key_pem.to_string(),
        })
    }

    #[must_use]
    pub fn fingerprint(&self) -> Fingerprint {
        Fingerprint::of_der(&self.cert)
    }

    /// Cadena de certificados para presentar en el handshake TLS (un único
    /// certificado autofirmado, sin intermedios).
    #[must_use]
    pub fn cert_chain(&self) -> Vec<CertificateDer<'static>> {
        vec![self.cert.clone()]
    }

    #[must_use]
    pub fn key(&self) -> PrivateKeyDer<'static> {
        self.key.clone_key()
    }

    #[must_use]
    pub fn cert_pem(&self) -> &str {
        &self.cert_pem
    }

    #[must_use]
    pub fn key_pem(&self) -> &str {
        &self.key_pem
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_identity_has_stable_fingerprint() {
        let identity = Identity::generate().expect("la generación no debería fallar");
        assert_eq!(identity.fingerprint(), identity.fingerprint());
    }

    #[test]
    fn two_generated_identities_have_different_fingerprints() {
        let a = Identity::generate().expect("la generación no debería fallar");
        let b = Identity::generate().expect("la generación no debería fallar");
        assert_ne!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn round_trips_through_pem() {
        let original = Identity::generate().expect("la generación no debería fallar");
        let reloaded = Identity::from_pem(original.cert_pem(), original.key_pem())
            .expect("recargar un PEM válido no debería fallar");
        assert_eq!(original.fingerprint(), reloaded.fingerprint());
    }
}
