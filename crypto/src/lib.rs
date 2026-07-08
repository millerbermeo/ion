//! Criptografía de `IonConnect`: identidad TLS, fingerprint TOFU y
//! construcción de configuración `rustls`.
//!
//! Todo el tráfico entre equipos viaja sobre TLS 1.3 (`rustls`, backend
//! `ring`). No se implementa AES-GCM ni intercambio de claves a mano: TLS 1.3
//! ya cubre ambos con una librería auditada, y duplicarlo a mano solo
//! añadiría superficie de bugs. En vez de una cadena de CA (no hay CA en un
//! escenario de KVM local), la confianza es TOFU: el fingerprint SHA-256 del
//! certificado de un equipo se acepta una vez y se recuerda — igual que
//! `known_hosts` de SSH.

mod config;
mod error;
mod fingerprint;
mod identity;
mod policy;
mod trust_store;
mod verifier;

pub use config::{client_config, server_config};
pub use error::CryptoError;
pub use fingerprint::Fingerprint;
pub use identity::Identity;
pub use policy::PairingMode;
pub use trust_store::{InMemoryTrustStore, TrustStore};
pub use verifier::{TofuClientVerifier, TofuServerVerifier};
