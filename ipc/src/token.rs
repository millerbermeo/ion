use std::fmt;
use std::path::Path;

use rand::Rng;

use crate::error::IpcError;

/// Token de autenticación local, de un solo uso por arranque del servicio.
/// No protege contra un atacante remoto (el socket es loopback, nunca sale
/// de la máquina) sino contra otro usuario local del mismo equipo — de ahí
/// que el archivo se guarde con permisos `0600`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct IpcToken([u8; 32]);

impl IpcToken {
    #[must_use]
    pub fn generate() -> Self {
        let mut bytes = [0u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    fn to_hex(self) -> String {
        use std::fmt::Write as _;

        self.0.iter().fold(String::with_capacity(64), |mut acc, b| {
            let _ = write!(acc, "{b:02x}");
            acc
        })
    }

    fn from_hex(hex: &str) -> Option<Self> {
        if hex.len() != 64 {
            return None;
        }
        let mut bytes = [0u8; 32];
        for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
            let byte_str = std::str::from_utf8(chunk).ok()?;
            bytes[i] = u8::from_str_radix(byte_str, 16).ok()?;
        }
        Some(Self(bytes))
    }

    /// Comparación en tiempo constante — evita que un temporizador de
    /// comparación byte a byte filtre cuántos bytes iniciales acertó un
    /// intento no autorizado.
    #[must_use]
    pub fn constant_time_eq(&self, other: &Self) -> bool {
        let mut diff = 0u8;
        for (a, b) in self.0.iter().zip(other.0.iter()) {
            diff |= a ^ b;
        }
        diff == 0
    }
}

impl fmt::Debug for IpcToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "IpcToken(<redactado>)")
    }
}

/// Datos publicados por el servidor para que un cliente local (la GUI)
/// pueda encontrarlo: el puerto en el que escucha y el token esperado.
#[derive(Debug, Clone, Copy)]
pub struct IpcEndpoint {
    pub port: u16,
    pub token: IpcToken,
}

impl IpcEndpoint {
    /// # Errors
    ///
    /// Devuelve [`IpcError::Io`] si no se pudo escribir `path`.
    #[cfg(unix)]
    pub fn save(&self, path: &Path) -> Result<(), IpcError> {
        use std::os::unix::fs::PermissionsExt;

        let contents = format!("{}\n{}\n", self.port, self.token.to_hex());
        std::fs::write(path, contents)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        Ok(())
    }

    /// # Errors
    ///
    /// Devuelve [`IpcError::Io`] si no se pudo escribir `path`.
    #[cfg(not(unix))]
    pub fn save(&self, path: &Path) -> Result<(), IpcError> {
        let contents = format!("{}\n{}\n", self.port, self.token.to_hex());
        std::fs::write(path, contents)?;
        Ok(())
    }

    /// # Errors
    ///
    /// Devuelve [`IpcError::Io`] si no se pudo leer `path`, o
    /// [`IpcError::MalformedTokenFile`] si su contenido no tiene el
    /// formato esperado (puerto y token hexadecimal en líneas separadas).
    pub fn load(path: &Path) -> Result<Self, IpcError> {
        let contents = std::fs::read_to_string(path)?;
        let mut lines = contents.lines();
        let port: u16 = lines
            .next()
            .and_then(|line| line.parse().ok())
            .ok_or(IpcError::MalformedTokenFile)?;
        let token = IpcToken::from_hex(lines.next().ok_or(IpcError::MalformedTokenFile)?)
            .ok_or(IpcError::MalformedTokenFile)?;
        Ok(Self { port, token })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_generated_tokens_differ() {
        assert_ne!(IpcToken::generate().0, IpcToken::generate().0);
    }

    #[test]
    fn hex_round_trip() {
        let token = IpcToken::generate();
        let hex = token.to_hex();
        assert_eq!(IpcToken::from_hex(&hex), Some(token));
    }

    #[test]
    fn constant_time_eq_detects_equality_and_difference() {
        let token = IpcToken::generate();
        assert!(token.constant_time_eq(&token));
        let other = IpcToken::generate();
        assert!(!token.constant_time_eq(&other));
    }

    #[test]
    fn endpoint_round_trips_through_a_file() {
        let dir = std::env::temp_dir().join(format!("ionconnect-ipc-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("crear el directorio temporal");
        let path = dir.join("ipc.token");

        let endpoint = IpcEndpoint {
            port: 54321,
            token: IpcToken::generate(),
        };
        endpoint.save(&path).expect("guardar no debería fallar");
        let loaded = IpcEndpoint::load(&path).expect("cargar no debería fallar");

        assert_eq!(loaded.port, endpoint.port);
        assert!(loaded.token.constant_time_eq(&endpoint.token));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
