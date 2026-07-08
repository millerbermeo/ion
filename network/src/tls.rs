use std::net::SocketAddr;
use std::sync::Arc;

use rustls::{ClientConfig, ServerConfig};
use rustls_pki_types::ServerName;
use tokio::net::TcpStream;
use tokio_rustls::{TlsAcceptor, TlsConnector, client, server};

use crate::connection::Connection;
use crate::error::NetworkError;

/// El nombre de servidor TLS es irrelevante para la verificación: TOFU se
/// basa en el fingerprint del certificado, no en el hostname. rustls exige
/// un `ServerName` igualmente, así que se usa siempre el mismo valor fijo.
const TLS_SERVER_NAME: &str = "ionconnect.local";

/// Conecta por TCP a `addr` y sube la conexión a TLS 1.3 como cliente.
///
/// # Errors
///
/// Devuelve [`NetworkError::Io`] si falla la conexión TCP o el handshake TLS
/// (incluyendo el rechazo por fingerprint no confiable, ver `crypto`).
///
/// # Panics
///
/// Nunca debería entrar en pánico: `TLS_SERVER_NAME` es una constante fija y
/// válida como nombre DNS.
pub async fn connect_tls(
    addr: SocketAddr,
    client_config: Arc<ClientConfig>,
) -> Result<Connection<client::TlsStream<TcpStream>>, NetworkError> {
    let tcp = TcpStream::connect(addr).await?;
    tcp.set_nodelay(true)?;
    let server_name = ServerName::try_from(TLS_SERVER_NAME)
        .expect("\"ionconnect.local\" es un nombre DNS válido");
    let tls = TlsConnector::from(client_config)
        .connect(server_name, tcp)
        .await?;
    Ok(Connection::new(tls))
}

/// Acepta una conexión TCP ya establecida y la sube a TLS 1.3 como servidor.
///
/// # Errors
///
/// Devuelve [`NetworkError::Io`] si falla el handshake TLS (incluyendo el
/// rechazo por fingerprint de cliente no confiable, ver `crypto`).
pub async fn accept_tls(
    tcp: TcpStream,
    server_config: Arc<ServerConfig>,
) -> Result<Connection<server::TlsStream<TcpStream>>, NetworkError> {
    tcp.set_nodelay(true)?;
    let tls = TlsAcceptor::from(server_config).accept(tcp).await?;
    Ok(Connection::new(tls))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use ionconnect_crypto::{
        Identity, InMemoryTrustStore, PairingMode, TrustStore, client_config, server_config,
    };
    use ionconnect_protocol::{Message, MouseMove};
    use tokio::net::TcpListener;

    use super::*;

    #[tokio::test]
    async fn sends_a_message_over_a_real_mutual_tls_loopback_connection() {
        let client_identity = Identity::generate().expect("la generación no debería fallar");
        let server_identity = Identity::generate().expect("la generación no debería fallar");

        let client_trust = Arc::new(InMemoryTrustStore::new());
        client_trust.trust(server_identity.fingerprint());
        let server_trust = Arc::new(InMemoryTrustStore::new());
        server_trust.trust(client_identity.fingerprint());

        let client_cfg = client_config(&client_identity, client_trust, PairingMode::RejectUnknown)
            .expect("configuración de cliente válida");
        let server_cfg = server_config(&server_identity, server_trust, PairingMode::RejectUnknown)
            .expect("configuración de servidor válida");

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("no se pudo abrir el listener");
        let addr = listener.local_addr().expect("dirección local esperada");

        let server_task = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.expect("accept debería funcionar");
            let mut conn = accept_tls(tcp, server_cfg)
                .await
                .expect("el handshake TLS del servidor debería completarse");
            conn.recv()
                .await
                .expect("recv no debería fallar")
                .expect("se esperaba un mensaje")
        });

        let mut client_conn = connect_tls(addr, client_cfg)
            .await
            .expect("el handshake TLS del cliente debería completarse");
        client_conn
            .send(&Message::MouseMove(MouseMove { x: 3, y: 4 }))
            .await
            .expect("send no debería fallar");

        let received = server_task
            .await
            .expect("el servidor no debería entrar en panic");
        assert_eq!(received, Message::MouseMove(MouseMove { x: 3, y: 4 }));
    }
}
