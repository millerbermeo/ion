/// Errores del crate de criptografía: generación de identidad, parseo de PEM
/// y construcción de configuración TLS. No incluye errores de framing/red
/// (esos viven en `protocol` y `network`).
#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("no se pudo generar el certificado autofirmado: {0}")]
    CertificateGeneration(#[from] rcgen::Error),

    #[error("no se pudo interpretar el PEM de certificado o clave: {0}")]
    Pem(#[from] rustls_pki_types::pem::Error),

    #[error("no se pudo construir la configuración TLS: {0}")]
    Tls(#[from] rustls::Error),
}
