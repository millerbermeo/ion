//! Eventos de transferencia para el aviso flotante ("toast") de la GUI.
//!
//! `core` corre como hijo de la GUI, que lee su stdout línea a línea. Cada
//! cambio de estado de una transferencia sale como una línea propia: el
//! prefijo [`MARKER`] seguido de un objeto JSON. No se deduce del texto de
//! los logs a propósito: así no depende del nivel de `RUST_LOG` elegido (con
//! `error` los `info!` no salen) ni de cómo `tracing` cita los campos.
//!
//! Solo se emiten si la GUI lo pidió con `IONCONNECT_GUI_EVENTS=1` — corriendo
//! `core` a mano en una terminal no ensucian la salida.

use std::fmt::Write as _;
use std::io::Write as _;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use ionconnect_protocol::FileBatchKind;

/// Prefijo de las líneas de evento en stdout. La GUI lo busca al principio
/// de la línea (ver `gui/src-tauri/src/commands.rs`).
pub const MARKER: &str = "@@ionconnect-transfer ";

/// Variable de entorno con la que la GUI activa los eventos.
pub const ENV_ENABLE: &str = "IONCONNECT_GUI_EVENTS";

/// Cada cuánto, como mucho, se reporta progreso — más seguido solo gasta
/// CPU de la GUI sin que el ojo note la diferencia.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(150);

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

static ENABLED: LazyLock<bool> =
    LazyLock::new(|| std::env::var(ENV_ENABLE).is_ok_and(|v| v == "1"));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Send,
    Receive,
}

/// Cómo se originó la transferencia, para el texto del aviso.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    /// Envío explícito desde la ventana de la app.
    App,
    /// Arrastre con el mouse cruzando el borde.
    Drag,
    /// Ctrl+C → cruzar el borde → Ctrl+V.
    Clipboard,
    /// Recepción que termina en la carpeta `ionconnect` del Escritorio.
    Folder,
}

impl Via {
    /// Origen de un envío según el tipo de lote ([`None`] = desde la app).
    #[must_use]
    pub fn for_send(kind: Option<FileBatchKind>) -> Self {
        match kind {
            None => Self::App,
            Some(FileBatchKind::Downloads) => Self::Drag,
            Some(FileBatchKind::Clipboard) => Self::Clipboard,
        }
    }

    /// Destino de una recepción según el tipo de lote.
    #[must_use]
    pub fn for_receive(kind: Option<FileBatchKind>) -> Self {
        match kind {
            Some(FileBatchKind::Clipboard) => Self::Clipboard,
            _ => Self::Folder,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::App => "app",
            Self::Drag => "drag",
            Self::Clipboard => "clipboard",
            Self::Folder => "folder",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Start,
    Progress,
    Done,
    Error,
}

impl Phase {
    fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Progress => "progress",
            Self::Done => "done",
            Self::Error => "error",
        }
    }
}

/// Una transferencia vista por el aviso de la GUI: emite `start` al crearse,
/// `progress` (con tope de frecuencia) a medida que avanza y `done`/`error`
/// al cerrarse. Si se descarta sin cerrarse —la sesión se cortó, la tarea
/// se canceló— emite `error` en el `Drop`, así el aviso nunca queda girando
/// para siempre.
#[derive(Debug)]
pub struct TransferToast {
    id: u64,
    direction: Direction,
    via: Via,
    name: String,
    count: usize,
    /// Bytes totales, `0` si no se conocen (recepción de un lote).
    total: u64,
    done: u64,
    /// Entradas que no se pudieron leer/escribir: el lote sigue, pero al
    /// final se reporta como error.
    failures: usize,
    last_progress: Instant,
    closed: bool,
}

impl TransferToast {
    #[must_use]
    pub fn start(direction: Direction, via: Via, name: &str, count: usize, total: u64) -> Self {
        let toast = Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            direction,
            via,
            name: name.to_string(),
            count,
            total,
            done: 0,
            failures: 0,
            last_progress: Instant::now(),
            closed: false,
        };
        toast.emit(Phase::Start, "");
        toast
    }

    /// Suma `bytes` transferidos; reporta progreso como mucho cada
    /// [`PROGRESS_INTERVAL`].
    pub fn advance(&mut self, bytes: u64) {
        self.done += bytes;
        if self.last_progress.elapsed() >= PROGRESS_INTERVAL {
            self.last_progress = Instant::now();
            self.emit(Phase::Progress, "");
        }
    }

    pub fn set_count(&mut self, count: usize) {
        self.count = count;
    }

    pub fn note_failure(&mut self) {
        self.failures += 1;
    }

    /// Cierra con éxito — o con error si alguna entrada falló por el camino.
    pub fn finish(mut self) {
        self.closed = true;
        if self.failures > 0 {
            let reason = if self.failures == 1 {
                "1 archivo no se pudo transferir".to_string()
            } else {
                format!("{} archivos no se pudieron transferir", self.failures)
            };
            self.emit(Phase::Error, &reason);
        } else {
            self.emit(Phase::Done, "");
        }
    }

    pub fn fail(mut self, reason: &str) {
        self.closed = true;
        self.emit(Phase::Error, reason);
    }

    fn emit(&self, phase: Phase, reason: &str) {
        if !*ENABLED {
            return;
        }
        let line = self.to_json(phase, reason);
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{MARKER}{line}");
        let _ = out.flush();
    }

    fn to_json(&self, phase: Phase, reason: &str) -> String {
        let direction = match self.direction {
            Direction::Send => "send",
            Direction::Receive => "receive",
        };
        format!(
            "{{\"id\":{},\"direction\":\"{direction}\",\"phase\":\"{}\",\"via\":\"{}\",\
             \"name\":{},\"count\":{},\"total\":{},\"done\":{},\"reason\":{}}}",
            self.id,
            phase.as_str(),
            self.via.as_str(),
            json_string(&self.name),
            self.count,
            self.total,
            self.done,
            json_string(reason),
        )
    }
}

impl Drop for TransferToast {
    fn drop(&mut self) {
        if !self.closed {
            self.emit(Phase::Error, "Se interrumpió la conexión");
        }
    }
}

/// `s` como literal de cadena JSON (con comillas). Los nombres de archivo
/// pueden traer cualquier cosa, incluidos `"` y caracteres de control.
fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if u32::from(c) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_string_escapes_quotes_backslashes_and_control_chars() {
        assert_eq!(json_string("foto.png"), "\"foto.png\"");
        assert_eq!(json_string("a\"b\\c"), "\"a\\\"b\\\\c\"");
        assert_eq!(json_string("x\ny\u{1}"), "\"x\\ny\\u0001\"");
        assert_eq!(json_string("año ñ"), "\"año ñ\"");
    }

    #[test]
    fn event_line_is_a_single_json_object() {
        let mut toast = TransferToast::start(Direction::Send, Via::Drag, "mi \"doc\".pdf", 2, 10);
        toast.advance(4);
        let json = toast.to_json(Phase::Progress, "");
        assert!(!json.contains('\n'));
        assert!(json.starts_with('{') && json.ends_with('}'));
        assert!(json.contains("\"direction\":\"send\""));
        assert!(json.contains("\"via\":\"drag\""));
        assert!(json.contains("\"name\":\"mi \\\"doc\\\".pdf\""));
        assert!(json.contains("\"done\":4"));
        toast.finish();
    }
}
