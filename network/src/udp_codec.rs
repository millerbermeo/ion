//! Cifrado/framing de los datagramas UDP que llevan los `MouseMove`
//! continuos (ver `core::udp_peers` para el porqué de tener un transporte
//! aparte de la conexión TCP+TLS). Este módulo es agnóstico de política de
//! red — no sabe de sockets ni de sesiones, solo transforma
//! `(seq, x, y)` hacia/desde bytes cifrados con una clave que trae quien
//! llama.
//!
//! Formato del datagrama: `[seq: u32 LE][ciphertext(8) + tag(16)]`. El
//! nonce no viaja por la red — se deriva determinísticamente de `seq`
//! (mismo truco que el packet number de QUIC): como la clave es nueva en
//! cada sesión TLS y `seq` es único y monótono dentro de esa clave, un
//! nonce aleatorio no aporta nada y solo suma 12 bytes de overhead. `seq`
//! viaja en claro pero autenticado como *associated data*, así que el
//! receptor puede rechazar un paquete viejo/repetido sin gastar en
//! descifrar.

use ring::aead::{Aad, CHACHA20_POLY1305, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};

use crate::error::NetworkError;

/// Clave simétrica para cifrar/descifrar datagramas UDP de una sesión —
/// se deriva vía `export_keying_material` sobre la conexión TLS ya
/// autenticada (ver `core::server`/`core::client`), no acá.
pub struct UdpKey(LessSafeKey);

impl UdpKey {
    /// # Errors
    ///
    /// Devuelve [`NetworkError::UdpCrypto`] si `key_bytes` no tiene el
    /// largo que espera `ChaCha20-Poly1305` (no debería pasar: siempre se
    /// alimenta con los 32 bytes que devuelve `export_keying_material`).
    pub fn new(key_bytes: &[u8; 32]) -> Result<Self, NetworkError> {
        let unbound =
            UnboundKey::new(&CHACHA20_POLY1305, key_bytes).map_err(|_| NetworkError::UdpCrypto)?;
        Ok(Self(LessSafeKey::new(unbound)))
    }
}

fn nonce_from_seq(seq: u32) -> Nonce {
    let mut bytes = [0u8; NONCE_LEN];
    bytes[NONCE_LEN - 4..].copy_from_slice(&seq.to_le_bytes());
    Nonce::assume_unique_for_key(bytes)
}

/// Cifra `(x, y)` bajo `seq` y arma el datagrama completo listo para
/// mandar por el socket UDP.
///
/// # Panics
///
/// Nunca debería entrar en pánico en uso normal: el único caso en que
/// `ring` puede fallar acá es un buffer/clave mal formados, y `payload`
/// siempre tiene el tamaño correcto para `ChaCha20-Poly1305`.
#[must_use]
pub fn seal_mouse_move(key: &UdpKey, seq: u32, x: i32, y: i32) -> Vec<u8> {
    let seq_bytes = seq.to_le_bytes();

    let mut payload = Vec::with_capacity(8 + CHACHA20_POLY1305.tag_len());
    payload.extend_from_slice(&x.to_le_bytes());
    payload.extend_from_slice(&y.to_le_bytes());

    key.0
        .seal_in_place_append_tag(nonce_from_seq(seq), Aad::from(seq_bytes), &mut payload)
        .expect("sellar con buffer bien formado y clave válida no debería fallar");

    let mut datagram = Vec::with_capacity(4 + payload.len());
    datagram.extend_from_slice(&seq_bytes);
    datagram.extend_from_slice(&payload);
    datagram
}

/// Verifica el tag y descifra un datagrama recibido, devolviendo
/// `(seq, x, y)`. No decide si `seq` es "suficientemente nuevo" — eso es
/// responsabilidad de quien llama (ver [`is_newer`]), porque requiere el
/// estado de la última secuencia aceptada, que este módulo no guarda.
///
/// # Errors
///
/// Devuelve [`NetworkError::UdpTruncated`] si el datagrama es más corto que
/// el mínimo esperado, o [`NetworkError::UdpCrypto`] si el tag de
/// autenticación no coincide (datagrama corrupto, forjado, o cifrado con
/// otra clave).
///
/// # Panics
///
/// Nunca debería entrar en pánico: el chequeo de largo mínimo de arriba ya
/// garantiza que `seq_bytes` tiene exactamente 4 bytes para el
/// `try_into()`.
pub fn open_mouse_move(key: &UdpKey, datagram: &[u8]) -> Result<(u32, i32, i32), NetworkError> {
    let min_len = 4 + 8 + CHACHA20_POLY1305.tag_len();
    if datagram.len() < min_len {
        return Err(NetworkError::UdpTruncated);
    }

    let (seq_bytes, sealed) = datagram.split_at(4);
    let seq_bytes: [u8; 4] = seq_bytes
        .try_into()
        .expect("split_at(4) garantiza exactamente 4 bytes");
    let seq = u32::from_le_bytes(seq_bytes);

    let mut buf = sealed.to_vec();
    let plaintext = key
        .0
        .open_in_place(nonce_from_seq(seq), Aad::from(seq_bytes), &mut buf)
        .map_err(|_| NetworkError::UdpCrypto)?;

    if plaintext.len() != 8 {
        return Err(NetworkError::UdpTruncated);
    }
    let x = i32::from_le_bytes(plaintext[0..4].try_into().expect("4 bytes exactos"));
    let y = i32::from_le_bytes(plaintext[4..8].try_into().expect("4 bytes exactos"));
    Ok((seq, x, y))
}

/// `true` si `seq` es más nuevo que `last_seen`, con aritmética de número
/// de serie ([RFC 1982](https://www.rfc-editor.org/rfc/rfc1982)) en vez de
/// una comparación directa — necesario porque el contador de secuencia es
/// un `u32` que en un servicio de fondo de larga duración eventualmente da
/// la vuelta; con `seq <= last_seen` ingenuo, ese único wraparound
/// descartaría todo paquete futuro para siempre.
#[must_use]
pub const fn is_newer(seq: u32, last_seen: u32) -> bool {
    seq.wrapping_sub(last_seen).cast_signed() > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> UdpKey {
        UdpKey::new(&[7u8; 32]).expect("32 bytes es el largo correcto")
    }

    #[test]
    fn seals_and_opens_a_round_trip() {
        let key = key();
        let datagram = seal_mouse_move(&key, 1, -120, 800);
        let (seq, x, y) = open_mouse_move(&key, &datagram).expect("debería abrir bien");
        assert_eq!((seq, x, y), (1, -120, 800));
    }

    #[test]
    fn rejects_a_datagram_sealed_with_a_different_key() {
        let sender = UdpKey::new(&[1u8; 32]).unwrap();
        let receiver = UdpKey::new(&[2u8; 32]).unwrap();
        let datagram = seal_mouse_move(&sender, 1, 10, 20);
        let err = open_mouse_move(&receiver, &datagram).unwrap_err();
        assert!(matches!(err, NetworkError::UdpCrypto));
    }

    #[test]
    fn rejects_a_tampered_datagram() {
        let key = key();
        let mut datagram = seal_mouse_move(&key, 1, 10, 20);
        let last = datagram.len() - 1;
        datagram[last] ^= 0xFF;
        let err = open_mouse_move(&key, &datagram).unwrap_err();
        assert!(matches!(err, NetworkError::UdpCrypto));
    }

    #[test]
    fn rejects_a_truncated_datagram() {
        let key = key();
        let err = open_mouse_move(&key, &[1, 2, 3]).unwrap_err();
        assert!(matches!(err, NetworkError::UdpTruncated));
    }

    #[test]
    fn different_sequences_do_not_decrypt_with_each_others_nonce() {
        // Mismo payload, seq distinta ⇒ nonce distinto ⇒ ciphertext distinto;
        // si se intenta abrir con la seq equivocada (nonce/AAD que no
        // coinciden con los que se usaron para sellar), tiene que fallar.
        let key = key();
        let datagram = seal_mouse_move(&key, 5, 1, 2);
        let mut wrong_seq = datagram.clone();
        wrong_seq[0..4].copy_from_slice(&6u32.to_le_bytes());
        assert!(open_mouse_move(&key, &wrong_seq).is_err());
    }

    #[test]
    fn serial_arithmetic_treats_normal_progress_as_newer() {
        assert!(is_newer(5, 4));
        assert!(!is_newer(4, 4));
        assert!(!is_newer(4, 5));
    }

    #[test]
    fn serial_arithmetic_survives_u32_wraparound() {
        assert!(is_newer(0, u32::MAX));
        assert!(is_newer(1, u32::MAX));
        assert!(!is_newer(u32::MAX, 0));
    }
}
