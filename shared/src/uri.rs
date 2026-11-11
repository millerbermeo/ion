//! Conversión entre rutas locales y URIs `file://` (RFC 8089), el formato en
//! el que viajan las listas de archivos por el portapapeles y por
//! arrastrar-y-soltar en Linux.

use std::path::{Path, PathBuf};

/// `true` para los caracteres que una URI admite sin escapar en una ruta.
const fn is_unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/')
}

/// `/home/ana/informe final.pdf` → `file:///home/ana/informe%20final.pdf`.
#[must_use]
pub fn path_to_file_uri(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let mut uri = String::with_capacity(raw.len() + 8);
    uri.push_str("file://");
    for byte in raw.bytes() {
        if is_unreserved(byte) {
            uri.push(char::from(byte));
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}

/// Inversa de [`path_to_file_uri`]. `None` si `uri` no es un `file://` local
/// (otro esquema, o un host remoto) o si el escapado está mal formado.
#[must_use]
pub fn file_uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.trim().strip_prefix("file://")?;
    // `file://host/ruta`: solo se acepta sin host o con `localhost`.
    let path = if rest.starts_with('/') {
        rest
    } else {
        let (host, _) = rest.split_once('/')?;
        if !host.eq_ignore_ascii_case("localhost") {
            return None;
        }
        &rest[host.len()..]
    };

    let bytes = path.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    Some(PathBuf::from(String::from_utf8(decoded).ok()?))
}

/// Rutas locales de un `text/uri-list` (RFC 2483): una URI por línea, las
/// que empiezan con `#` son comentarios. Lo que no sea un archivo local se
/// descarta.
#[must_use]
pub fn paths_from_uri_list(list: &str) -> Vec<PathBuf> {
    list.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(file_uri_to_path)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_paths_with_spaces_and_unicode() {
        for raw in ["/home/ana/informe final.pdf", "/tmp/ñandú (1)#x?.txt", "/a/b%c"] {
            let path = PathBuf::from(raw);
            let uri = path_to_file_uri(&path);
            assert!(uri.is_ascii(), "la URI debe quedar en ASCII: {uri}");
            assert_eq!(file_uri_to_path(&uri), Some(path));
        }
    }

    #[test]
    fn accepts_localhost_and_rejects_remote_or_foreign_schemes() {
        assert_eq!(
            file_uri_to_path("file://localhost/etc/hosts"),
            Some(PathBuf::from("/etc/hosts"))
        );
        assert_eq!(file_uri_to_path("file://servidor/compartido/x"), None);
        assert_eq!(file_uri_to_path("https://ejemplo.com/x"), None);
        assert_eq!(file_uri_to_path("file:///mal%2"), None);
    }

    #[test]
    fn parses_a_uri_list_skipping_comments_and_blanks() {
        let list = "# comentario\r\nfile:///a/uno.txt\r\n\r\nfile:///b/dos%20tres.txt\r\nhttp://x/y\r\n";
        assert_eq!(
            paths_from_uri_list(list),
            vec![PathBuf::from("/a/uno.txt"), PathBuf::from("/b/dos tres.txt")]
        );
    }
}
