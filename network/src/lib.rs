//! Transporte de `IonConnect`: framing sobre TCP+TLS con `tokio`, heartbeat,
//! reconexión con backoff exponencial y descubrimiento mDNS.
//!
//! Todo orientado a eventos — nada de polling. La criptografía (TLS, TOFU)
//! vive en `crypto`; este crate solo la envuelve con `tokio-rustls` para
//! obtener un `Connection` asíncrono.

mod backoff;
mod codec;
mod connection;
mod discovery;
mod error;
mod heartbeat;
mod tls;

pub use backoff::{Backoff, BackoffPolicy, connect_with_backoff};
pub use codec::MessageCodec;
pub use connection::Connection;
pub use discovery::{DiscoveredPeer, Discovery, peer_from_event};
pub use error::NetworkError;
pub use heartbeat::HeartbeatMonitor;
pub use tls::{accept_tls, connect_tls};
