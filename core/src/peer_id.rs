use ionconnect_shared::DeviceId;

/// Convierte un `DeviceId` a hexadecimal de 32 caracteres — el formato en
/// el que `config::PeerConfig::device_id` se guarda en el TOML (legible y
/// editable a mano, a diferencia del formato UUID con guiones de
/// `DeviceId::Display`).
#[must_use]
pub fn to_hex(device_id: DeviceId) -> String {
    device_id
        .as_bytes()
        .iter()
        .fold(String::with_capacity(32), |mut acc, byte| {
            use std::fmt::Write as _;
            let _ = write!(acc, "{byte:02x}");
            acc
        })
}

/// # Errors
///
/// Devuelve `None` si `hex` no son exactamente 32 caracteres hexadecimales.
#[must_use]
pub fn from_hex(hex: &str) -> Option<DeviceId> {
    let hex = hex.trim();
    if hex.len() != 32 {
        return None;
    }
    let mut bytes = [0u8; 16];
    for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
        let byte_str = std::str::from_utf8(chunk).ok()?;
        bytes[i] = u8::from_str_radix(byte_str, 16).ok()?;
    }
    Some(DeviceId::from_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_hex() {
        let id = DeviceId::new();
        let hex = to_hex(id);
        assert_eq!(hex.len(), 32);
        assert_eq!(from_hex(&hex), Some(id));
    }

    #[test]
    fn rejects_malformed_hex() {
        assert_eq!(from_hex("no es hex"), None);
        assert_eq!(from_hex("abc"), None);
    }
}
