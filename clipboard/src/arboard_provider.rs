use std::path::PathBuf;

use arboard::{Clipboard, Error as ArboardError};

use crate::error::ClipboardError;
use crate::provider::ClipboardProvider;

/// Adaptador sobre `arboard`, que ya cubre Windows/X11/Wayland con una sola
/// API, evitando repetir por sistema operativo lo que ese crate resuelve
/// bien.
pub struct ArboardProvider {
    clipboard: Clipboard,
}

impl ArboardProvider {
    /// # Errors
    ///
    /// Devuelve [`ClipboardError`] si no se pudo abrir el portapapeles del
    /// sistema (por ejemplo, sin servidor de display disponible).
    pub fn new() -> Result<Self, ClipboardError> {
        let clipboard = Clipboard::new().map_err(|e| ClipboardError::Backend(e.to_string()))?;
        Ok(Self { clipboard })
    }
}

impl ClipboardProvider for ArboardProvider {
    fn get_text(&mut self) -> Result<Option<String>, ClipboardError> {
        match self.clipboard.get_text() {
            Ok(text) => Ok(Some(text)),
            // El portapapeles vacío o con contenido no textual (p. ej. una
            // imagen) no es un error de nuestro dominio, solo "no hay texto".
            Err(ArboardError::ContentNotAvailable) => Ok(None),
            Err(err) => Err(ClipboardError::Backend(err.to_string())),
        }
    }

    fn set_text(&mut self, text: &str) -> Result<(), ClipboardError> {
        self.clipboard
            .set_text(text.to_string())
            .map_err(|e| ClipboardError::Backend(e.to_string()))
    }

    fn get_files(&mut self) -> Result<Option<Vec<PathBuf>>, ClipboardError> {
        match self.clipboard.get().file_list() {
            Ok(files) => {
                let files: Vec<PathBuf> = files.into_iter().filter_map(normalize_path).collect();
                Ok(if files.is_empty() { None } else { Some(files) })
            }
            // Sin lista de archivos (texto, imagen, vacío) o con una que no
            // se pudo interpretar: para nosotros es "no hay archivos".
            Err(ArboardError::ContentNotAvailable | ArboardError::ConversionFailure) => Ok(None),
            Err(err) => Err(ClipboardError::Backend(err.to_string())),
        }
    }

    fn set_files(&mut self, files: &[PathBuf]) -> Result<(), ClipboardError> {
        #[cfg(all(unix, not(target_os = "macos")))]
        if prefers_x11_owner() {
            match crate::x11_files::publish(files) {
                Ok(()) => return Ok(()),
                Err(err) => {
                    tracing::debug!(%err, "dueño X11 propio no disponible, se usa arboard");
                }
            }
        }
        self.clipboard
            .set()
            .file_list(files)
            .map_err(|e| ClipboardError::Backend(e.to_string()))
    }
}

/// `true` cuando conviene publicar los archivos con nuestro propio dueño X11
/// (ver `x11_files`) en vez de con `arboard`: en una sesión X11, y también en
/// GNOME sobre Wayland — ahí `arboard` igual termina usando `XWayland`
/// (mutter no ofrece el protocolo `data-control`) y es justo donde Nautilus
/// necesita el formato extra. En los demás compositores Wayland `arboard`
/// habla Wayland nativo, que es lo correcto.
#[cfg(all(unix, not(target_os = "macos")))]
fn prefers_x11_owner() -> bool {
    if std::env::var_os("DISPLAY").is_none() {
        return false;
    }
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let gnome = std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|desktop| {
        desktop
            .split(':')
            .any(|part| part.eq_ignore_ascii_case("gnome"))
    });
    !wayland || gnome
}

/// Limpia una ruta leída del portapapeles. `arboard` parte los
/// `text/uri-list` solo por `\n`, así que con las listas terminadas en CRLF
/// —lo que manda el estándar y lo que emiten GTK/Nautilus— cada ruta llega
/// con un `\r` pegado al final y ya no existe en disco. Las rutas vacías se
/// descartan.
fn normalize_path(path: PathBuf) -> Option<PathBuf> {
    let Some(raw) = path.to_str() else {
        return Some(path);
    };
    let trimmed = raw.trim_end_matches(['\r', '\n']);
    if trimmed.is_empty() {
        None
    } else if trimmed.len() == raw.len() {
        Some(path)
    } else {
        Some(PathBuf::from(trimmed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_path_strips_the_carriage_return_left_by_crlf_lists() {
        assert_eq!(
            normalize_path(PathBuf::from("/tmp/a b.txt\r")),
            Some(PathBuf::from("/tmp/a b.txt"))
        );
        assert_eq!(
            normalize_path(PathBuf::from("/tmp/ok.txt")),
            Some(PathBuf::from("/tmp/ok.txt"))
        );
        assert_eq!(normalize_path(PathBuf::from("\r")), None);
    }
}
