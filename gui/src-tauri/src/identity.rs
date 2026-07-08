use std::path::Path;

use ionconnect_crypto::{CryptoError, Identity};

/// Carga o genera la identidad TLS del equipo, igual que `core` — mismo
/// directorio de configuración, mismo par de archivos. Duplicado a
/// propósito en vez de compartir código con `core` (que es un binario sin
/// biblioteca pública); es poca lógica y ambos deben coincidir
/// exactamente en cómo derivan el `device_id` para que lo que la GUI
/// muestra sea lo mismo que `core` realmente usa.
///
/// # Errors
///
/// Devuelve [`CryptoError`] si falla la generación o el parseo del PEM.
pub fn load_or_generate_identity(dir: &Path) -> Result<Identity, CryptoError> {
    let cert_path = dir.join("identity.crt");
    let key_path = dir.join("identity.key");

    if cert_path.exists() && key_path.exists() {
        let cert_pem = std::fs::read_to_string(&cert_path).unwrap_or_default();
        let key_pem = std::fs::read_to_string(&key_path).unwrap_or_default();
        Identity::from_pem(&cert_pem, &key_pem)
    } else {
        let identity = Identity::generate()?;
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(&cert_path, identity.cert_pem());
        let _ = std::fs::write(&key_path, identity.key_pem());
        Ok(identity)
    }
}

/// Hexadecimal de 32 caracteres del `device_id` derivado del fingerprint de
/// la identidad — el mismo formato que espera `config::PeerConfig::device_id`.
#[must_use]
pub fn device_id_hex(identity: &Identity) -> String {
    use std::fmt::Write as _;

    identity.fingerprint().as_bytes()[..16].iter().fold(
        String::with_capacity(32),
        |mut acc, byte| {
            let _ = write!(acc, "{byte:02x}");
            acc
        },
    )
}
