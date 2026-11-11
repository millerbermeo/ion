//! Aviso flotante ("toast") de transferencias de archivos.
//!
//! Las transferencias por arrastre o Ctrl+C/Ctrl+V pasan mientras el usuario
//! trabaja en otras apps: la ventana principal casi nunca está a la vista,
//! así que un toast dentro de ella no lo vería nadie. Por eso el aviso vive
//! en una ventana propia — sin bordes, transparente, siempre encima, fuera
//! de la barra de tareas, que **no acepta foco** (mostrarla no le roba el
//! teclado a la app en la que el usuario está escribiendo) y que deja pasar
//! los clicks (`set_ignore_cursor_events`) para no tapar nada.
//!
//! `core` reporta cada cambio de estado como una línea `MARKER + JSON` en su
//! stdout (`core/src/transfer_events.rs`); `commands::stream_output` la
//! reenvía acá. La ventana se muestra al llegar un evento y el propio
//! frontend pide ocultarla (`toast_idle`) cuando ya no le quedan avisos.
//!
//! En una sesión Wayland nativa el compositor no deja posicionar ventanas:
//! ahí el aviso aparece donde el compositor decida.

use std::sync::atomic::{AtomicU64, Ordering};

use tauri::{
    AppHandle, LogicalSize, Manager, PhysicalPosition, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder,
};

/// Prefijo de las líneas de evento de `core` — debe coincidir con
/// `transfer_events::MARKER` de `ionconnect-core`.
pub const MARKER: &str = "@@ionconnect-transfer ";

const LABEL: &str = "toast";
/// Tamaño lógico de la ventana: entra una pila de ~4 avisos. Lo que no
/// ocupan los avisos es transparente y deja pasar los clicks.
const WIDTH: f64 = 380.0;
const HEIGHT: f64 = 440.0;
/// Separación lógica del borde de la pantalla.
const MARGIN: f64 = 12.0;

/// Secuencia del último evento entregado al frontend. `toast_idle` solo
/// oculta la ventana si el frontend vio el último evento — si llegó otro en
/// el medio, el pedido de ocultar quedó viejo.
static SEQ: AtomicU64 = AtomicU64::new(0);

/// Crea la ventana del aviso, oculta. Se llama una vez al arrancar la app.
///
/// # Errors
///
/// Devuelve el error de Tauri si no se pudo crear la ventana.
pub fn create_window(app: &AppHandle) -> tauri::Result<()> {
    let window = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("toast.html".into()))
        .title("IonConnect")
        .inner_size(WIDTH, HEIGHT)
        .resizable(false)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible_on_all_workspaces(true)
        .focused(false)
        .focusable(false)
        .visible(false)
        .build()?;
    // OJO: no llamar `set_ignore_cursor_events` acá. Con la ventana oculta
    // (sin realizar) tao hace `unwrap` del `gdk::Window` y la app entera
    // entra en panic al arrancar. Se aplica tras `show()` en `handle_event`.
    drop(window);
    Ok(())
}

/// Entrega a la ventana del aviso un evento de transferencia (`json`, el
/// resto de la línea después de [`MARKER`]) y la muestra si estaba oculta.
pub fn handle_event(app: &AppHandle, json: &str) {
    // Se re-serializa en vez de pegar el texto tal cual en el `eval`: así
    // lo que llega al JS es siempre un literal JSON válido, nunca código.
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return;
    };
    if !value.is_object() {
        return;
    }
    let Some(window) = app.get_webview_window(LABEL) else {
        return;
    };
    let seq = SEQ.fetch_add(1, Ordering::SeqCst) + 1;
    if !window.is_visible().unwrap_or(false) {
        place(app, &window);
        let _ = window.show();
        let _ = window.set_ignore_cursor_events(true);
    }
    let _ = window.eval(format!(
        "window.__ionTransfer && window.__ionTransfer({value}, {seq})"
    ));
}

/// Esquina superior derecha del área de trabajo (sin paneles ni barras) del
/// monitor donde está el puntero — donde el usuario está mirando —, o del
/// principal si no se puede saber.
fn place(app: &AppHandle, window: &WebviewWindow) {
    let monitor = app
        .cursor_position()
        .ok()
        .and_then(|p| app.monitor_from_point(p.x, p.y).ok().flatten())
        .or_else(|| app.primary_monitor().ok().flatten());
    let Some(monitor) = monitor else {
        return;
    };
    let scale = monitor.scale_factor();
    let _ = window.set_size(LogicalSize::new(WIDTH, HEIGHT));
    let area = monitor.work_area();
    let width = WIDTH * scale;
    let margin = MARGIN * scale;
    let right = f64::from(area.position.x) + f64::from(area.size.width);
    #[allow(clippy::cast_possible_truncation)]
    let position = PhysicalPosition::new(
        (right - width - margin).round() as i32,
        (f64::from(area.position.y) + margin).round() as i32,
    );
    let _ = window.set_position(position);
}

/// El frontend del aviso ya no tiene nada en pantalla: se oculta la ventana,
/// salvo que haya llegado un evento después del último que vio (`seq`).
#[tauri::command]
pub fn toast_idle(app: AppHandle, seq: u64) {
    if seq != SEQ.load(Ordering::SeqCst) {
        return;
    }
    if let Some(window) = app.get_webview_window(LABEL) {
        let _ = window.hide();
    }
}
