use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Identificador único y estable de un equipo dentro de la red `IonConnect`.
///
/// Se genera una vez por instalación y se persiste en `config/` — no cambia
/// entre reinicios, para que el pairing TOFU pueda asociar un fingerprint de
/// certificado a un `DeviceId` concreto.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DeviceId(Uuid);

impl DeviceId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    #[must_use]
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(Uuid::from_bytes(bytes))
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        self.0.as_bytes()
    }
}

impl Default for DeviceId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_bytes() {
        let id = DeviceId::new();
        let bytes = *id.as_bytes();
        assert_eq!(DeviceId::from_bytes(bytes), id);
    }

    #[test]
    fn two_new_ids_are_distinct() {
        assert_ne!(DeviceId::new(), DeviceId::new());
    }
}
