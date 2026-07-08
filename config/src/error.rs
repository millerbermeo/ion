/// Errores de carga/guardado/observación del archivo de configuración.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("error de E/S: {0}")]
    Io(#[from] std::io::Error),

    #[error("error al interpretar TOML: {0}")]
    Deserialize(#[from] toml::de::Error),

    #[error("error al serializar TOML: {0}")]
    Serialize(#[from] toml::ser::Error),

    #[error("error observando cambios en el archivo de configuración: {0}")]
    Watch(String),
}
