//! IPC local entre la GUI y el servicio `core` de `IonConnect`.
//!
//! En vez de mantener un segundo stack de transporte, se reutiliza el
//! mismo framing/protocolo binario de `network`/`protocol` sobre TCP en
//! loopback (`127.0.0.1`), con un token de un solo arranque como
//! autenticación local — el socket nunca sale de la máquina, así que TLS no
//! aporta nada aquí; lo que hace falta es que otro usuario local no pueda
//! conectarse, y eso lo resuelven el token + permisos `0600` del archivo
//! que lo contiene.

mod client;
mod error;
mod server;
mod token;

pub use client::IpcClient;
pub use error::IpcError;
pub use server::IpcServer;
pub use token::{IpcEndpoint, IpcToken};
