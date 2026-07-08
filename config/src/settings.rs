use std::path::Path;

use ionconnect_screen::ScreenEdge;
use serde::{Deserialize, Serialize};

use crate::error::ConfigError;

/// Política de confianza TOFU preferida, persistida por el usuario.
/// Espejo liviano de `ionconnect_crypto::PairingMode` — se mantiene
/// separado en vez de depender de `crypto` desde `config` solo para dos
/// variantes; la conversión es responsabilidad de quien conecte ambos
/// crates (`core`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PairingModePreference {
    AutoTrustOnFirstUse,
    RejectUnknown,
}

/// Rol de este equipo en la sesión de `IonConnect`.
///
/// `Server` es el equipo con el mouse/teclado físico: captura la entrada y
/// decide, según [`Layout`](ionconnect_screen::Layout), a cuál `Client` se
/// la reenvía cuando el cursor cruza un borde configurado. `Client` solo
/// recibe entrada y la inyecta localmente.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Role {
    Server,
    Client,
}

/// Un equipo vecino conocido, del punto de vista del `Server`: en qué borde
/// de la pantalla de *este* equipo hay que cruzar para cederle el control.
///
/// `device_id` se guarda en hexadecimal (no como el tipo `DeviceId` de
/// `shared` directamente) para que el TOML sea legible/editable a mano;
/// `core` hace la conversión al conectar con `crypto`/`protocol`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerConfig {
    pub device_id: String,
    pub name: String,
    pub edge: ScreenEdge,
}

/// Configuración persistente de una instalación de `IonConnect`.
///
/// `#[serde(default)]` en la estructura y en cada campo asegura que un TOML
/// parcial (o de una versión anterior que no conocía un campo nuevo) siga
/// cargando con valores razonables en vez de fallar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub device_name: String,
    pub listen_port: u16,
    pub discovery_enabled: bool,
    pub pairing_mode: PairingModePreference,
    pub log_level: String,
    pub role: Role,
    /// Solo relevante para `Role::Server`: a qué borde de esta pantalla
    /// corresponde cada equipo vecino.
    pub peers: Vec<PeerConfig>,
    /// Solo relevante para `Role::Client`: `host:puerto` del servidor al que
    /// conectarse. `None` para descubrirlo por mDNS en la LAN.
    pub server_address: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            device_name: "ionconnect".to_string(),
            listen_port: 44890,
            discovery_enabled: true,
            pairing_mode: PairingModePreference::RejectUnknown,
            log_level: "info".to_string(),
            role: Role::Server,
            peers: Vec::new(),
            server_address: None,
        }
    }
}

impl Settings {
    /// # Errors
    ///
    /// Devuelve [`ConfigError::Deserialize`] si `contents` no es TOML válido
    /// o no coincide con la forma esperada.
    pub fn from_toml_str(contents: &str) -> Result<Self, ConfigError> {
        Ok(toml::from_str(contents)?)
    }

    /// # Errors
    ///
    /// Devuelve [`ConfigError::Serialize`] si la serialización falla (no
    /// debería ocurrir para esta estructura, pero se propaga por si acaso).
    pub fn to_toml_string(&self) -> Result<String, ConfigError> {
        Ok(toml::to_string_pretty(self)?)
    }

    /// # Errors
    ///
    /// Devuelve [`ConfigError::Io`] si no se pudo leer `path`, o
    /// [`ConfigError::Deserialize`] si su contenido no es TOML válido.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        Self::from_toml_str(&std::fs::read_to_string(path)?)
    }

    /// # Errors
    ///
    /// Devuelve [`ConfigError::Io`] si no se pudo escribir `path`.
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        std::fs::write(path, self.to_toml_string()?)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_toml() {
        let settings = Settings {
            device_name: "escritorio-oficina".to_string(),
            listen_port: 12345,
            discovery_enabled: false,
            pairing_mode: PairingModePreference::AutoTrustOnFirstUse,
            log_level: "debug".to_string(),
            role: Role::Client,
            peers: vec![PeerConfig {
                device_id: "abc123".to_string(),
                name: "laptop-sala".to_string(),
                edge: ScreenEdge::Right,
            }],
            server_address: Some("192.168.1.10:44890".to_string()),
        };
        let toml_text = settings
            .to_toml_string()
            .expect("serializar no debería fallar");
        let reloaded = Settings::from_toml_str(&toml_text).expect("deserializar no debería fallar");
        assert_eq!(settings, reloaded);
    }

    #[test]
    fn partial_toml_falls_back_to_defaults_for_missing_fields() {
        let settings = Settings::from_toml_str(r#"device_name = "solo-nombre""#)
            .expect("un TOML parcial debería cargar igual");
        assert_eq!(settings.device_name, "solo-nombre");
        assert_eq!(settings.listen_port, Settings::default().listen_port);
        assert!(settings.discovery_enabled);
        assert_eq!(settings.role, Role::Server);
        assert!(settings.peers.is_empty());
    }

    #[test]
    fn empty_toml_yields_defaults() {
        assert_eq!(Settings::from_toml_str("").unwrap(), Settings::default());
    }
}
