use std::path::Path;

use ionconnect_network::Connection;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

use crate::error::IpcError;
use crate::token::IpcEndpoint;

/// Extremo de IPC local del lado GUI: lee el archivo de token que publicó
/// [`crate::IpcServer::bind`] y se conecta con él.
pub struct IpcClient;

impl IpcClient {
    /// # Errors
    ///
    /// Devuelve [`IpcError::Io`]/[`IpcError::MalformedTokenFile`] si no se
    /// pudo leer `token_file`, o [`IpcError::Io`] si falla la conexión TCP.
    pub async fn connect(token_file: &Path) -> Result<Connection<TcpStream>, IpcError> {
        let endpoint = IpcEndpoint::load(token_file)?;
        let mut stream = TcpStream::connect(("127.0.0.1", endpoint.port)).await?;
        stream.write_all(endpoint.token.as_bytes()).await?;
        Ok(Connection::new(stream))
    }
}

#[cfg(test)]
mod tests {
    use ionconnect_protocol::{Heartbeat, Message};
    use tokio::io::AsyncWriteExt as _;

    use crate::server::IpcServer;

    use super::*;

    fn token_path() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ionconnect-ipc-client-test-{}-{}",
            std::process::id(),
            rand::random::<u32>()
        ));
        std::fs::create_dir_all(&dir).expect("crear el directorio temporal");
        dir.join("ipc.token")
    }

    #[tokio::test]
    async fn client_authenticates_and_exchanges_a_message() {
        let path = token_path();
        let server = IpcServer::bind(&path)
            .await
            .expect("bind no debería fallar");

        let server_task = tokio::spawn(async move {
            let mut conn = server.accept().await.expect("accept no debería fallar");
            conn.recv()
                .await
                .expect("recv no debería fallar")
                .expect("se esperaba un mensaje")
        });

        let mut client_conn = IpcClient::connect(&path)
            .await
            .expect("connect no debería fallar");
        client_conn
            .send(Message::Heartbeat(Heartbeat { sequence: 1 }))
            .await
            .expect("send no debería fallar");

        let received = server_task
            .await
            .expect("el servidor no debería entrar en panic");
        assert_eq!(received, Message::Heartbeat(Heartbeat { sequence: 1 }));
    }

    #[tokio::test]
    async fn connection_with_wrong_token_is_ignored_by_the_server() {
        let path = token_path();
        let server = IpcServer::bind(&path)
            .await
            .expect("bind no debería fallar");
        let addr = server.local_addr().expect("dirección local esperada");

        let server_task = tokio::spawn(async move {
            let mut conn = server.accept().await.expect("accept no debería fallar");
            conn.recv()
                .await
                .expect("recv no debería fallar")
                .expect("se esperaba un mensaje")
        });

        // Un cliente sin el token correcto: 32 bytes en cero.
        let mut impostor = TcpStream::connect(addr)
            .await
            .expect("connect no debería fallar");
        impostor
            .write_all(&[0u8; 32])
            .await
            .expect("escribir el token falso no debería fallar");
        drop(impostor);

        // El cliente legítimo sí debería lograr pasar la autenticación.
        let mut client_conn = IpcClient::connect(&path)
            .await
            .expect("connect no debería fallar");
        client_conn
            .send(Message::Heartbeat(Heartbeat { sequence: 2 }))
            .await
            .expect("send no debería fallar");

        let received = server_task
            .await
            .expect("el servidor no debería entrar en panic");
        assert_eq!(received, Message::Heartbeat(Heartbeat { sequence: 2 }));
    }
}
