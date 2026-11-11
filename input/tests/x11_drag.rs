//! Pruebas del robo de arrastre (`x11::take_dragged_files`) contra
//! aplicaciones GTK **reales** corriendo en un servidor X real: lo que se
//! valida es que el protocolo XDND que hablamos es el que entienden los
//! toolkits, no una reimplementación nuestra del otro lado.
//!
//! `#[ignore]`: mueven el cursor, aprietan botones e inyectan Escape. Correr
//! solo en un display anidado:
//!
//! ```sh
//! Xephyr :80 -screen 1024x768 -ac &
//! DISPLAY=:80 cargo test -p ionconnect-input --test x11_drag -- --ignored --test-threads=1
//! ```
//!
//! Necesitan `python3-gi` con GTK 3 y GTK 4 (vienen con cualquier escritorio
//! GNOME); si falta, la prueba correspondiente falla al no ver `READY`.
#![cfg(all(unix, not(target_os = "macos")))]

use std::io::{BufRead as _, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use ionconnect_input::x11::{X11Injector, take_dragged_files};
use ionconnect_input::{CapturedEvent, InputInjector};
use ionconnect_protocol::MouseButton;

/// Origen de arrastre GTK 3: una ventana sin decorar en (0,0) cuyo contenido
/// entero arrastra los archivos pasados por argumento.
const GTK3_SOURCE: &str = r"
import sys, gi
gi.require_version('Gtk', '3.0'); gi.require_version('Gdk', '3.0')
from gi.repository import Gtk, Gdk, Gio
uris = [Gio.File.new_for_path(p).get_uri() for p in sys.argv[1:]]
win = Gtk.Window(type=Gtk.WindowType.POPUP)
win.set_default_size(300, 300); win.move(0, 0)
box = Gtk.EventBox(); win.add(box)
if uris:
    box.drag_source_set(Gdk.ModifierType.BUTTON1_MASK, [], Gdk.DragAction.COPY)
    box.drag_source_add_uri_targets()
    box.connect('drag-data-get', lambda w, c, sel, i, t: sel.set_uris(uris))
else:
    box.drag_source_set(Gdk.ModifierType.BUTTON1_MASK, [], Gdk.DragAction.COPY)
    box.drag_source_add_text_targets()
    box.connect('drag-data-get', lambda w, c, sel, i, t: sel.set_text('solo texto', -1))
box.connect('drag-begin', lambda *a: print('BEGIN', flush=True))
box.connect('drag-end', lambda *a: print('END', flush=True))
win.show_all(); print('READY', flush=True); Gtk.main()
";

/// Lo mismo con GTK 4, que reescribió todo su XDND.
const GTK4_SOURCE: &str = r"
import sys, gi
gi.require_version('Gtk', '4.0'); gi.require_version('Gdk', '4.0')
from gi.repository import Gtk, Gdk, Gio, GLib, GObject
files = [Gio.File.new_for_path(p) for p in sys.argv[1:]]
win = Gtk.Window(); win.set_decorated(False); win.set_default_size(300, 300)
box = Gtk.Box(); box.set_hexpand(True); box.set_vexpand(True); win.set_child(box)
source = Gtk.DragSource(); source.set_actions(Gdk.DragAction.COPY)
value = GObject.Value(Gdk.FileList, Gdk.FileList.new_from_list(files))
source.set_content(Gdk.ContentProvider.new_for_value(value))
source.connect('drag-begin', lambda *a: print('BEGIN', flush=True))
source.connect('drag-end', lambda *a: print('END', flush=True))
source.connect('drag-cancel', lambda *a: (print('END', flush=True), False)[1])
box.add_controller(source)
win.present(); GLib.timeout_add(300, lambda: print('READY', flush=True))
GLib.MainLoop().run()
";

/// Una aplicación GTK de prueba; se mata al salir de alcance.
struct Source {
    child: Child,
    lines: Receiver<String>,
}

impl Source {
    fn spawn(script: &str, files: &[PathBuf]) -> Self {
        let mut child = Command::new("python3")
            .arg("-c")
            .arg(script)
            .args(files)
            // Fuerza el backend X11 aunque la sesión del desarrollador sea
            // Wayland: lo que se prueba es XDND.
            .env("GDK_BACKEND", "x11")
            .env_remove("WAYLAND_DISPLAY")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("python3 debería estar instalado");
        let stdout = child.stdout.take().expect("stdout capturado");
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let source = Self { child, lines };
        source.expect_line("READY", Duration::from_secs(15));
        // Margen para que la ventana termine de mapearse.
        std::thread::sleep(Duration::from_millis(300));
        source
    }

    /// `true` si la aplicación imprime `wanted` antes de `timeout`.
    fn saw_line(&self, wanted: &str, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(line) if line.trim() == wanted => return true,
                Ok(_) => {}
                Err(_) => return false,
            }
        }
    }

    fn expect_line(&self, wanted: &str, timeout: Duration) {
        assert!(
            self.saw_line(wanted, timeout),
            "la aplicación GTK de prueba no imprimió `{wanted}` a tiempo"
        );
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn pointer(injector: &mut X11Injector, x: i32, y: i32) {
    injector
        .inject(&CapturedEvent::MouseMove { x, y })
        .expect("mover el mouse");
    std::thread::sleep(Duration::from_millis(40));
}

fn left_button(injector: &mut X11Injector, pressed: bool) {
    injector
        .inject(&CapturedEvent::MouseButton {
            button: MouseButton::Left,
            pressed,
        })
        .expect("botón izquierdo");
    std::thread::sleep(Duration::from_millis(60));
}

/// Aprieta dentro de la ventana de prueba y se aleja lo suficiente como para
/// superar el umbral de arrastre del toolkit, terminando fuera de ella.
fn start_drag(injector: &mut X11Injector) {
    pointer(injector, 150, 150);
    left_button(injector, true);
    for step in 1..=12 {
        pointer(injector, 150 + step * 20, 150 + step * 10);
    }
}

fn sample_files(tag: &str) -> (PathBuf, Vec<PathBuf>) {
    let dir = std::env::temp_dir().join(format!("ion drag {tag} {}", std::process::id()));
    std::fs::create_dir_all(dir.join("carpeta")).expect("crear carpeta de prueba");
    let file = dir.join("informe final ñ.txt");
    std::fs::write(&file, b"x").expect("crear archivo de prueba");
    let files = vec![file, dir.join("carpeta")];
    (dir, files)
}

/// Arrastre real de archivos: se devuelven esas rutas y el arrastre termina
/// cancelado en la aplicación de origen.
fn steals_a_real_file_drag(script: &str, tag: &str) {
    let (dir, files) = sample_files(tag);
    let source = Source::spawn(script, &files);
    let mut injector = X11Injector::connect().expect("XTEST debería estar disponible");

    start_drag(&mut injector);
    source.expect_line("BEGIN", Duration::from_secs(3));

    let dragged = take_dragged_files();
    assert_eq!(dragged.paths, files, "se esperaban los archivos arrastrados");
    assert_eq!(
        dragged.injected_escape,
        Some(1),
        "Escape es el keycode evdev 1"
    );
    // El arrastre tiene que haber terminado **con el botón todavía
    // apretado**: si recién terminara al soltar, se habría soltado sobre lo
    // que hubiera debajo del cursor.
    source.expect_line("END", Duration::from_secs(3));

    // Y la aplicación tiene que poder volver a arrastrar después: cancelar
    // no debe dejarle rota ninguna ventana auxiliar.
    left_button(&mut injector, false);
    start_drag(&mut injector);
    source.expect_line("BEGIN", Duration::from_secs(3));
    assert_eq!(take_dragged_files().paths, files, "segundo arrastre");
    source.expect_line("END", Duration::from_secs(3));

    left_button(&mut injector, false);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
#[ignore = "requiere un servidor X real (Xephyr/Xvfb) en $DISPLAY y python3-gi"]
fn steals_a_file_drag_from_a_gtk3_application() {
    steals_a_real_file_drag(GTK3_SOURCE, "gtk3");
}

#[test]
#[ignore = "requiere un servidor X real (Xephyr/Xvfb) en $DISPLAY y python3-gi"]
fn steals_a_file_drag_from_a_gtk4_application() {
    steals_a_real_file_drag(GTK4_SOURCE, "gtk4");
}

/// Un arrastre que no lleva archivos (texto) no se toca: ni se devuelve
/// nada ni se cancela.
#[test]
#[ignore = "requiere un servidor X real (Xephyr/Xvfb) en $DISPLAY y python3-gi"]
fn leaves_a_text_drag_alone() {
    let source = Source::spawn(GTK3_SOURCE, &[]);
    let mut injector = X11Injector::connect().expect("XTEST debería estar disponible");

    start_drag(&mut injector);
    source.expect_line("BEGIN", Duration::from_secs(3));

    let dragged = take_dragged_files();
    assert!(dragged.paths.is_empty());
    assert_eq!(dragged.injected_escape, None);
    assert!(
        !source.saw_line("END", Duration::from_millis(400)),
        "un arrastre de texto no debería cancelarse"
    );
    left_button(&mut injector, false);
}

/// El caso que motivó hacer el protocolo completo: después de un arrastre ya
/// terminado, la aplicación sigue siendo dueña de `XdndSelection`. Cruzar el
/// borde con el botón apretado por otro motivo no debe "resucitar" esos
/// archivos.
#[test]
#[ignore = "requiere un servidor X real (Xephyr/Xvfb) en $DISPLAY y python3-gi"]
fn ignores_the_stale_selection_of_a_finished_drag() {
    let (dir, files) = sample_files("viejo");
    let source = Source::spawn(GTK3_SOURCE, &files);
    let mut injector = X11Injector::connect().expect("XTEST debería estar disponible");

    // Un arrastre completo que se suelta en el vacío.
    start_drag(&mut injector);
    source.expect_line("BEGIN", Duration::from_secs(3));
    left_button(&mut injector, false);
    source.expect_line("END", Duration::from_secs(3));

    // Botón apretado fuera de la ventana, sin arrastre.
    pointer(&mut injector, 600, 500);
    left_button(&mut injector, true);
    let dragged = take_dragged_files();
    left_button(&mut injector, false);

    assert_eq!(dragged, ionconnect_input::x11::DraggedFiles::default());
    std::fs::remove_dir_all(dir).ok();
}
