use std::net::SocketAddr;
use std::path::Path;

use ionconnect_network::Connection;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tracing::warn;

use crate::error::IpcError;
use crate::token::{IpcEndpoint, IpcToken};

/// Extremo de IPC local del lado `core`. Escucha únicamente en loopback —
/// este socket nunca debe exponerse a la red — y exige el token de
/// [`IpcEndpoint`] antes de tratar la conexión como válida.
pub struct IpcServer {
    listener: TcpListener,
    token: IpcToken,
}

impl IpcServer {
    /// Abre un listener en un puerto efímero de `127.0.0.1` y publica
    /// puerto+token en `token_file` (con permisos `0600` en Unix) para que
    /// la GUI pueda encontrarlo.
    ///
    /// # Errors
    ///
    /// Devuelve [`IpcError::Io`] si no se pudo abrir el socket o escribir
    /// `token_file`.
    pub async fn bind(token_file: &Path) -> Result<Self, IpcError> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let port = listener.local_addr()?.port();
        let token = IpcToken::generate();
        IpcEndpoint { port, token }.save(token_file)?;
        Ok(Self { listener, token })
    }

    /// # Errors
    ///
    /// Devuelve [`IpcError::Io`] si falla la consulta de la dirección local.
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Acepta la próxima conexión autorizada. Cualquier conexión que no
    /// presente el token correcto en sus primeros 32 bytes se descarta en
    /// silencio (solo un `warn` en el log) y se sigue esperando — no se
    /// distingue hacia afuera un token incorrecto de una conexión que
    /// nunca llegó, para no dar pistas a quien esté probando al azar.
    ///
    /// # Errors
    ///
    /// Devuelve [`IpcError::Io`] si el propio `accept` del listener falla.
    pub async fn accept(&self) -> Result<Connection<TcpStream>, IpcError> {
        loop {
            let (mut stream, _addr) = self.listener.accept().await?;
            let mut presented = [0u8; 32];
            if stream.read_exact(&mut presented).await.is_err() {
                continue;
            }
            if !IpcToken::from_bytes(presented).constant_time_eq(&self.token) {
                warn!("conexión de IPC local rechazada: token inválido");
                let _ = stream.shutdown().await;
                continue;
            }
            return Ok(Connection::new(stream));
        }
    }
}
