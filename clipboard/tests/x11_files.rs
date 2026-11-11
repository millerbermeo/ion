//! Pruebas contra un servidor X real del portapapeles de archivos. Están
//! `#[ignore]`: publican en el portapapeles, así que se corren en un display
//! anidado y nunca contra la sesión del usuario:
//!
//! ```sh
//! Xephyr :80 -screen 800x600 -ac &
//! DISPLAY=:80 cargo test -p ionconnect-clipboard --test x11_files -- --ignored --test-threads=1
//! ```
#![cfg(all(unix, not(target_os = "macos")))]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ionconnect_clipboard::{ArboardProvider, ClipboardProvider};
use x11rb::connection::Connection as _;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, WindowClass,
};

/// Pide `target` de `CLIPBOARD` como lo haría cualquier otra aplicación y
/// devuelve los bytes, o `None` si el dueño dice que no lo tiene.
fn read_target(target: &str) -> Option<Vec<u8>> {
    let (conn, screen_num) = x11rb::connect(None).expect("conectar al servidor X");
    let root = conn.setup().roots[screen_num].root;
    let window = conn.generate_id().unwrap();
    conn.create_window(
        x11rb::COPY_DEPTH_FROM_PARENT,
        window,
        root,
        0,
        0,
        1,
        1,
        0,
        WindowClass::INPUT_OUTPUT,
        x11rb::COPY_FROM_PARENT,
        &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
    )
    .unwrap();
    let atom = |name: &str| {
        conn.intern_atom(false, name.as_bytes())
            .unwrap()
            .reply()
            .unwrap()
            .atom
    };
    let (clipboard, target, property) = (atom("CLIPBOARD"), atom(target), atom("ION_TEST"));
    conn.convert_selection(window, clipboard, target, property, x11rb::CURRENT_TIME)
        .unwrap();
    conn.flush().unwrap();

    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        match conn.poll_for_event().unwrap() {
            Some(Event::SelectionNotify(notify)) => {
                if notify.property == x11rb::NONE {
                    return None;
                }
                let reply = conn
                    .get_property(false, window, property, AtomEnum::ANY, 0, u32::MAX)
                    .unwrap()
                    .reply()
                    .unwrap();
                return Some(reply.value);
            }
            Some(_) => {}
            None => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    panic!("el dueño del portapapeles no respondió a tiempo");
}

fn sample() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/tmp/ion prueba/informe final.pdf"),
        PathBuf::from("/tmp/ion prueba/carpeta"),
    ]
}

#[test]
#[ignore = "requiere un servidor X real (Xephyr/Xvfb) en $DISPLAY"]
fn published_files_are_offered_in_every_file_manager_format() {
    let mut provider = ArboardProvider::new().expect("abrir el portapapeles");
    provider.set_files(&sample()).expect("publicar archivos");

    assert_eq!(
        String::from_utf8(read_target("text/uri-list").unwrap()).unwrap(),
        "file:///tmp/ion%20prueba/informe%20final.pdf\r\nfile:///tmp/ion%20prueba/carpeta\r\n"
    );
    assert_eq!(
        String::from_utf8(read_target("x-special/gnome-copied-files").unwrap()).unwrap(),
        "copy\nfile:///tmp/ion%20prueba/informe%20final.pdf\nfile:///tmp/ion%20prueba/carpeta"
    );
    assert_eq!(
        String::from_utf8(read_target("UTF8_STRING").unwrap()).unwrap(),
        "/tmp/ion prueba/informe final.pdf\n/tmp/ion prueba/carpeta"
    );
    assert_eq!(read_target("image/png"), None, "un formato que no ofrecemos");
    // TARGETS: 7 átomos de 4 bytes.
    assert_eq!(read_target("TARGETS").unwrap().len(), 7 * 4);
}

#[test]
#[ignore = "requiere un servidor X real (Xephyr/Xvfb) en $DISPLAY"]
fn published_files_read_back_as_the_same_paths() {
    let mut provider = ArboardProvider::new().expect("abrir el portapapeles");
    provider.set_files(&sample()).expect("publicar archivos");
    assert_eq!(provider.get_files().expect("leer"), Some(sample()));
}

#[test]
#[ignore = "requiere un servidor X real (Xephyr/Xvfb) en $DISPLAY"]
fn writing_text_afterwards_replaces_the_file_list() {
    let mut provider = ArboardProvider::new().expect("abrir el portapapeles");
    provider.set_files(&sample()).expect("publicar archivos");
    provider.set_text("hola").expect("escribir texto");
    assert_eq!(provider.get_files().expect("leer"), None);
    assert_eq!(provider.get_text().expect("leer"), Some("hola".to_string()));
}
