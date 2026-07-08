use std::fmt;

use rustls_pki_types::CertificateDer;
use sha2::{Digest, Sha256};

/// SHA-256 de un certificado X.509 en DER, usado como identidad TOFU.
///
/// No es un hash del contenido "humano" del certificado (subject, etc.) sino
/// de los bytes DER completos — cualquier cambio de clave pública o de campo
/// produce un fingerprint distinto, que es exactamente lo que queremos
/// detectar como posible MITM.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fingerprint([u8; 32]);

impl Fingerprint {
    #[must_use]
    pub fn of_der(cert: &CertificateDer<'_>) -> Self {
        let digest = Sha256::digest(cert.as_ref());
        Self(digest.into())
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in &self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Fingerprint({self})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_der_produces_same_fingerprint() {
        let der = CertificateDer::from(vec![1, 2, 3, 4]);
        assert_eq!(Fingerprint::of_der(&der), Fingerprint::of_der(&der));
    }

    #[test]
    fn different_der_produces_different_fingerprint() {
        let a = CertificateDer::from(vec![1, 2, 3, 4]);
        let b = CertificateDer::from(vec![1, 2, 3, 5]);
        assert_ne!(Fingerprint::of_der(&a), Fingerprint::of_der(&b));
    }

    #[test]
    fn displays_as_lowercase_hex() {
        let der = CertificateDer::from(vec![0xAB; 8]);
        let text = Fingerprint::of_der(&der).to_string();
        assert_eq!(text.len(), 64);
        assert!(
            text.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }
}
