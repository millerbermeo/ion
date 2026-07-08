use std::collections::HashSet;
use std::fmt::Debug;
use std::sync::RwLock;

use crate::fingerprint::Fingerprint;

/// Conjunto de fingerprints de certificados en los que ya confiamos.
///
/// Modelo mental: `known_hosts` de SSH, pero de certificados en vez de claves
/// de host. Este crate solo define el contrato; dónde y cómo se persiste
/// (archivo, base de datos, etc.) es responsabilidad de quien lo implemente
/// en una fase posterior (`config`/`network`). Los métodos toman `&self` (no
/// `&mut self`) porque los verificadores de rustls solo entregan una
/// referencia compartida.
pub trait TrustStore: Debug + Send + Sync {
    /// Indica si `fingerprint` ya es de confianza.
    fn is_trusted(&self, fingerprint: &Fingerprint) -> bool;

    /// Marca `fingerprint` como confiable a partir de ahora.
    fn trust(&self, fingerprint: Fingerprint);
}

/// Implementación de `TrustStore` en memoria, sin persistencia. Suficiente
/// para tests de este crate y como base para una implementación respaldada
/// por disco más adelante.
#[derive(Debug, Default)]
pub struct InMemoryTrustStore {
    trusted: RwLock<HashSet<Fingerprint>>,
}

impl InMemoryTrustStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl TrustStore for InMemoryTrustStore {
    fn is_trusted(&self, fingerprint: &Fingerprint) -> bool {
        self.trusted
            .read()
            .expect("el lock de lectura de InMemoryTrustStore está envenenado")
            .contains(fingerprint)
    }

    fn trust(&self, fingerprint: Fingerprint) {
        self.trusted
            .write()
            .expect("el lock de escritura de InMemoryTrustStore está envenenado")
            .insert(fingerprint);
    }
}

#[cfg(test)]
mod tests {
    use rustls_pki_types::CertificateDer;

    use super::*;

    #[test]
    fn unknown_fingerprint_is_not_trusted() {
        let store = InMemoryTrustStore::new();
        let fingerprint = Fingerprint::of_der(&CertificateDer::from(vec![1, 2, 3]));
        assert!(!store.is_trusted(&fingerprint));
    }

    #[test]
    fn trusted_fingerprint_is_remembered() {
        let store = InMemoryTrustStore::new();
        let fingerprint = Fingerprint::of_der(&CertificateDer::from(vec![1, 2, 3]));
        store.trust(fingerprint);
        assert!(store.is_trusted(&fingerprint));
    }
}
