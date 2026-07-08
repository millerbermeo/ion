use std::path::Path;

use ionconnect_crypto::Identity;
use ionconnect_shared::DeviceId;

use crate::error::CoreError;

/// Carga la identidad TLS persistida en `dir`, o genera una nueva y la
/// guarda ahí si es la primera vez que corre este equipo. Esto es lo que
/// mantiene estable el fingerprint (y por lo tanto el `DeviceId` derivado
/// de él) entre reinicios — sin esto, cada arranque parecería un equipo
/// distinto y el pairing TOFU se rompería.
///
/// # Errors
///
/// Devuelve [`CoreError::Io`] si falla la lectura/escritura de los
/// archivos, o [`CoreError::Crypto`] si la generación o el parseo del PEM
/// falla.
pub fn load_or_generate_identity(dir: &Path) -> Result<Identity, CoreError> {
    let cert_path = dir.join("identity.crt");
    let key_path = dir.join("identity.key");

    if cert_path.exists() && key_path.exists() {
        let cert_pem = std::fs::read_to_string(&cert_path)?;
        let key_pem = std::fs::read_to_string(&key_path)?;
        Ok(Identity::from_pem(&cert_pem, &key_pem)?)
    } else {
        let identity = Identity::generate()?;
        std::fs::create_dir_all(dir)?;
        std::fs::write(&cert_path, identity.cert_pem())?;
        std::fs::write(&key_path, identity.key_pem())?;
        Ok(identity)
    }
}

/// Deriva un `DeviceId` estable a partir del fingerprint de la identidad
/// TLS — evita mantener un identificador separado que podría desincronizarse
/// del certificado real. Toma los primeros 16 de los 32 bytes del
/// fingerprint SHA-256; siguen siendo, a todo efecto práctico, únicos por
/// equipo.
#[must_use]
pub fn local_device_id(identity: &Identity) -> DeviceId {
    let fingerprint = identity.fingerprint();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&fingerprint.as_bytes()[..16]);
    DeviceId::from_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_and_reloads_the_same_identity() {
        let dir = std::env::temp_dir().join(format!(
            "ionconnect-core-identity-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);

        let first = load_or_generate_identity(&dir).expect("generar no debería fallar");
        let second = load_or_generate_identity(&dir).expect("recargar no debería fallar");

        assert_eq!(first.fingerprint(), second.fingerprint());
        assert_eq!(local_device_id(&first), local_device_id(&second));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn different_identities_yield_different_device_ids() {
        let a = Identity::generate().expect("generar no debería fallar");
        let b = Identity::generate().expect("generar no debería fallar");
        assert_ne!(local_device_id(&a), local_device_id(&b));
    }
}
