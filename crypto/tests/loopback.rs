//! Pruebas de handshake TLS 1.3 mutuo sobre un loopback TCP real
//! (`127.0.0.1`), sin tokio: `crypto` no depende de un runtime async, así
//! que basta `std::net` síncrono para ejercitar exactamente la misma
//! `rustls::ClientConfig`/`ServerConfig` que `network` envolverá más
//! adelante con `tokio-rustls`.

use std::io;
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;

use ionconnect_crypto::{
    Identity, InMemoryTrustStore, PairingMode, TrustStore, client_config, server_config,
};
use rustls::{ClientConfig, ClientConnection, ServerConfig, ServerConnection};
use rustls_pki_types::ServerName;

/// Conecta `client_config` y `server_config` sobre un loopback TCP real y
/// corre el handshake hasta completarse (o fallar) en ambos lados.
fn handshake(
    client_config: Arc<ClientConfig>,
    server_config: Arc<ServerConfig>,
) -> (io::Result<()>, io::Result<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("no se pudo abrir el listener");
    let addr = listener
        .local_addr()
        .expect("el listener debería tener dirección local");

    let server_thread = thread::spawn(move || -> io::Result<()> {
        let (mut socket, _) = listener.accept()?;
        let mut conn = ServerConnection::new(server_config).map_err(io::Error::other)?;
        conn.complete_io(&mut socket)?;
        Ok(())
    });

    let client_result = (|| -> io::Result<()> {
        let mut socket = TcpStream::connect(addr)?;
        let server_name = ServerName::try_from("ionconnect.local")
            .expect("\"ionconnect.local\" es un nombre DNS válido");
        let mut conn =
            ClientConnection::new(client_config, server_name).map_err(io::Error::other)?;
        conn.complete_io(&mut socket)?;
        Ok(())
    })();

    let server_result = server_thread
        .join()
        .expect("el hilo del servidor no debería entrar en panic");

    (client_result, server_result)
}

#[test]
fn mutual_tls_succeeds_when_fingerprints_are_pre_trusted() {
    let client_identity = Identity::generate().expect("la generación no debería fallar");
    let server_identity = Identity::generate().expect("la generación no debería fallar");

    let client_trust_store = Arc::new(InMemoryTrustStore::new());
    client_trust_store.trust(server_identity.fingerprint());
    let server_trust_store = Arc::new(InMemoryTrustStore::new());
    server_trust_store.trust(client_identity.fingerprint());

    let client_cfg = client_config(
        &client_identity,
        client_trust_store,
        PairingMode::RejectUnknown,
    )
    .expect("la configuración de cliente debería construirse");
    let server_cfg = server_config(
        &server_identity,
        server_trust_store,
        PairingMode::RejectUnknown,
    )
    .expect("la configuración de servidor debería construirse");

    let (client_result, server_result) = handshake(client_cfg, server_cfg);
    assert!(client_result.is_ok(), "cliente: {client_result:?}");
    assert!(server_result.is_ok(), "servidor: {server_result:?}");
}

#[test]
fn handshake_fails_when_server_fingerprint_is_unknown() {
    let client_identity = Identity::generate().expect("la generación no debería fallar");
    let server_identity = Identity::generate().expect("la generación no debería fallar");

    // El cliente nunca vio el fingerprint del servidor.
    let client_trust_store = Arc::new(InMemoryTrustStore::new());
    let server_trust_store = Arc::new(InMemoryTrustStore::new());
    server_trust_store.trust(client_identity.fingerprint());

    let client_cfg = client_config(
        &client_identity,
        client_trust_store,
        PairingMode::RejectUnknown,
    )
    .expect("la configuración de cliente debería construirse");
    let server_cfg = server_config(
        &server_identity,
        server_trust_store,
        PairingMode::RejectUnknown,
    )
    .expect("la configuración de servidor debería construirse");

    let (client_result, _server_result) = handshake(client_cfg, server_cfg);
    assert!(
        client_result.is_err(),
        "el cliente debió rechazar un fingerprint de servidor desconocido"
    );
}

#[test]
fn handshake_fails_when_client_fingerprint_is_unknown() {
    let client_identity = Identity::generate().expect("la generación no debería fallar");
    let server_identity = Identity::generate().expect("la generación no debería fallar");

    let client_trust_store = Arc::new(InMemoryTrustStore::new());
    client_trust_store.trust(server_identity.fingerprint());
    // El servidor nunca vio el fingerprint del cliente.
    let server_trust_store = Arc::new(InMemoryTrustStore::new());

    let client_cfg = client_config(
        &client_identity,
        client_trust_store,
        PairingMode::RejectUnknown,
    )
    .expect("la configuración de cliente debería construirse");
    let server_cfg = server_config(
        &server_identity,
        server_trust_store,
        PairingMode::RejectUnknown,
    )
    .expect("la configuración de servidor debería construirse");

    let (_client_result, server_result) = handshake(client_cfg, server_cfg);
    assert!(
        server_result.is_err(),
        "el servidor debió rechazar un fingerprint de cliente desconocido"
    );
}

#[test]
fn auto_trust_on_first_use_then_rejects_impostor_with_different_key() {
    let client_identity = Identity::generate().expect("la generación no debería fallar");
    let server_identity = Identity::generate().expect("la generación no debería fallar");

    let client_trust_store = Arc::new(InMemoryTrustStore::new());
    let server_trust_store = Arc::new(InMemoryTrustStore::new());

    // Primer contacto: nadie conoce a nadie, pero estamos en ventana de
    // emparejamiento (AutoTrustOnFirstUse), así que ambos fingerprints
    // quedan memorizados tras el primer handshake exitoso.
    let first_client_cfg = client_config(
        &client_identity,
        Arc::clone(&client_trust_store) as Arc<dyn TrustStore>,
        PairingMode::AutoTrustOnFirstUse,
    )
    .expect("la configuración de cliente debería construirse");
    let first_server_cfg = server_config(
        &server_identity,
        Arc::clone(&server_trust_store) as Arc<dyn TrustStore>,
        PairingMode::AutoTrustOnFirstUse,
    )
    .expect("la configuración de servidor debería construirse");

    let (first_client_result, first_server_result) = handshake(first_client_cfg, first_server_cfg);
    assert!(first_client_result.is_ok(), "{first_client_result:?}");
    assert!(first_server_result.is_ok(), "{first_server_result:?}");
    assert!(client_trust_store.is_trusted(&server_identity.fingerprint()));

    // Un impostor con OTRA clave intenta hacerse pasar por el mismo
    // servidor. Ahora operamos en modo estricto: el fingerprint no
    // coincide con el memorizado, así que el cliente debe rechazarlo.
    let impostor_identity = Identity::generate().expect("la generación no debería fallar");
    let second_client_cfg = client_config(
        &client_identity,
        Arc::clone(&client_trust_store) as Arc<dyn TrustStore>,
        PairingMode::RejectUnknown,
    )
    .expect("la configuración de cliente debería construirse");
    let second_server_cfg = server_config(
        &impostor_identity,
        Arc::clone(&server_trust_store) as Arc<dyn TrustStore>,
        PairingMode::RejectUnknown,
    )
    .expect("la configuración de servidor debería construirse");

    let (second_client_result, _second_server_result) =
        handshake(second_client_cfg, second_server_cfg);
    assert!(
        second_client_result.is_err(),
        "el cliente debió rechazar un certificado de servidor con fingerprint distinto al memorizado"
    );
}
