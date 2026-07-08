use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::RwLock;

use ionconnect_crypto::{Fingerprint, TrustStore};

/// `TrustStore` respaldado en un archivo de texto plano (un fingerprint
/// hexadecimal por línea) — el análogo de `known_hosts` de SSH que
/// `ionconnect_crypto` deja explícitamente para que quien lo use decida
/// cómo persistirlo. Sin esto, cada reinicio de `core` olvidaría todos los
/// equipos ya emparejados.
#[derive(Debug)]
pub struct FileTrustStore {
    path: PathBuf,
    trusted: RwLock<HashSet<Fingerprint>>,
}

impl FileTrustStore {
    /// Carga los fingerprints ya confiados desde `path` (si existe); un
    /// archivo ausente se trata como "todavía no hay nadie confiado", no
    /// como un error.
    ///
    /// # Errors
    ///
    /// Devuelve [`std::io::Error`] si `path` existe pero no se pudo leer.
    pub fn load(path: PathBuf) -> std::io::Result<Self> {
        let trusted = if path.exists() {
            std::fs::read_to_string(&path)?
                .lines()
                .filter_map(parse_hex_fingerprint)
                .collect()
        } else {
            HashSet::new()
        };
        Ok(Self {
            path,
            trusted: RwLock::new(trusted),
        })
    }

    fn persist(&self) {
        let Ok(trusted) = self.trusted.read() else {
            return;
        };
        let contents = trusted
            .iter()
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        if let Err(err) = std::fs::write(&self.path, contents) {
            tracing::warn!(%err, "no se pudo persistir el trust store en disco");
        }
    }
}

impl TrustStore for FileTrustStore {
    fn is_trusted(&self, fingerprint: &Fingerprint) -> bool {
        self.trusted
            .read()
            .expect("el lock de lectura del trust store no debería estar envenenado")
            .contains(fingerprint)
    }

    fn trust(&self, fingerprint: Fingerprint) {
        {
            let mut trusted = self
                .trusted
                .write()
                .expect("el lock de escritura del trust store no debería estar envenenado");
            trusted.insert(fingerprint);
        }
        self.persist();
    }
}

fn parse_hex_fingerprint(line: &str) -> Option<Fingerprint> {
    let line = line.trim();
    if line.len() != 64 {
        return None;
    }
    let mut bytes = [0u8; 32];
    for (i, chunk) in line.as_bytes().chunks(2).enumerate() {
        let byte_str = std::str::from_utf8(chunk).ok()?;
        bytes[i] = u8::from_str_radix(byte_str, 16).ok()?;
    }
    Some(Fingerprint::from_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use rustls_pki_types::CertificateDer;

    use super::*;

    fn store_path() -> PathBuf {
        std::env::temp_dir().join(format!(
            "ionconnect-core-trust-test-{}-{}",
            std::process::id(),
            rand_suffix()
        ))
    }

    fn rand_suffix() -> u128 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("el reloj no debería estar antes de 1970")
            .as_nanos()
    }

    #[test]
    fn missing_file_starts_empty() {
        let store = FileTrustStore::load(store_path()).expect("cargar no debería fallar");
        let fingerprint = Fingerprint::of_der(&CertificateDer::from(vec![1, 2, 3]));
        assert!(!store.is_trusted(&fingerprint));
    }

    #[test]
    fn trust_persists_across_reloads() {
        let path = store_path();
        let fingerprint = Fingerprint::of_der(&CertificateDer::from(vec![9, 9, 9]));

        {
            let store = FileTrustStore::load(path.clone()).expect("cargar no debería fallar");
            store.trust(fingerprint);
        }

        let reloaded = FileTrustStore::load(path.clone()).expect("recargar no debería fallar");
        assert!(reloaded.is_trusted(&fingerprint));

        let _ = std::fs::remove_file(&path);
    }
}
