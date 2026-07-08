use futures_util::{SinkExt, StreamExt};
use ionconnect_protocol::Message;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::Framed;

use crate::codec::MessageCodec;
use crate::error::NetworkError;

/// Conexión `IonConnect` ya establecida sobre cualquier transporte que
/// implemente `AsyncRead + AsyncWrite` (en producción, un `TlsStream` de
/// `tokio-rustls`; en tests, un `TcpStream` simple basta para ejercitar el
/// framing sin necesitar TLS real).
pub struct Connection<T> {
    framed: Framed<T, MessageCodec>,
}

impl<T: AsyncRead + AsyncWrite + Unpin> Connection<T> {
    #[must_use]
    pub fn new(io: T) -> Self {
        Self {
            framed: Framed::new(io, MessageCodec::default()),
        }
    }

    /// Envía un mensaje. Retorna cuando el mensaje ya fue entregado al
    /// socket subyacente (no implica que el peer lo haya procesado).
    ///
    /// # Errors
    ///
    /// Devuelve [`NetworkError`] si falla la codificación o la escritura.
    pub async fn send(&mut self, message: &Message) -> Result<(), NetworkError> {
        self.framed.send(message.clone()).await
    }

    /// Espera el próximo mensaje. `Ok(None)` significa que el peer cerró la
    /// conexión ordenadamente.
    ///
    /// # Errors
    ///
    /// Devuelve [`NetworkError`] si falla la lectura o la decodificación.
    pub async fn recv(&mut self) -> Result<Option<Message>, NetworkError> {
        self.framed.next().await.transpose()
    }
}

#[cfg(test)]
mod tests {
    use ionconnect_protocol::{Heartbeat, MouseMove};
    use tokio::net::{TcpListener, TcpStream};

    use super::*;

    #[tokio::test]
    async fn round_trips_messages_over_real_tcp_loopback() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("no se pudo abrir el listener");
        let addr = listener.local_addr().expect("dirección local esperada");

        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.expect("accept debería funcionar");
            let mut conn = Connection::new(socket);
            let first = conn.recv().await.expect("recv no debería fallar");
            conn.send(&Message::Heartbeat(Heartbeat { sequence: 7 }))
                .await
                .expect("send no debería fallar");
            first
        });

        let mut client = Connection::new(
            TcpStream::connect(addr)
                .await
                .expect("connect debería funcionar"),
        );
        client
            .send(&Message::MouseMove(MouseMove { x: 12, y: -8 }))
            .await
            .expect("send no debería fallar");
        let reply = client
            .recv()
            .await
            .expect("recv no debería fallar")
            .expect("se esperaba un mensaje");

        let received_by_server = server
            .await
            .expect("el servidor no debería entrar en panic");

        assert_eq!(
            received_by_server,
            Some(Message::MouseMove(MouseMove { x: 12, y: -8 }))
        );
        assert_eq!(reply, Message::Heartbeat(Heartbeat { sequence: 7 }));
    }
}
