//! Detecta un arrastre de archivos en curso (XDND) y se lo "roba" a la
//! aplicación de origen: averigua qué archivos lleva el usuario y cancela el
//! arrastre local, para que `core` los mande al otro equipo.
//!
//! No alcanza con mirar si la selección `XdndSelection` tiene dueño: las
//! aplicaciones la conservan después de terminar un arrastre, así que un
//! dueño viejo respondería con los archivos de un arrastre de hace una hora
//! en cuanto el usuario cruzara el borde seleccionando texto. La única
//! prueba fiable de que hay un arrastre **ahora** es que el origen nos hable
//! a nosotros, y eso solo pasa si somos un destino válido bajo el puntero.
//! Por eso se hace el protocolo XDND completo del lado destino:
//!
//! 1. Se mapea una ventanita `XdndAware` justo bajo el puntero.
//! 2. Se mueve el puntero un píxel de ida y vuelta: los orígenes buscan el
//!    destino en cada evento de movimiento, y contra un borde de pantalla el
//!    movimiento físico ya no genera ninguno.
//! 3. Un origen con un arrastre activo responde `XdndEnter` + `XdndPosition`.
//! 4. Se le pide `text/uri-list` por `XdndSelection`.
//! 5. Con la lista en mano se cancela el arrastre local — ver
//!    [`cancel_drag`] para el cómo, que depende del toolkit.
//!
//! Los archivos solo se devuelven si el arrastre efectivamente terminó: de
//! lo contrario el usuario los seguiría llevando en el cursor y además
//! aparecerían en el otro equipo.
//!
//! Funciona con aplicaciones X11 (incluidas las que corren sobre
//! `XWayland`). Un arrastre entre aplicaciones Wayland nativas no pasa por
//! el servidor X y es invisible desde acá.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ionconnect_shared::uri::paths_from_uri_list;
use x11rb::connection::{Connection as _, RequestConnection as _};
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ClientMessageEvent, ConfigureWindowAux, ConnectionExt as _, CreateWindowAux,
    EventMask, GrabMode, GrabStatus, KEY_PRESS_EVENT, KEY_RELEASE_EVENT, MOTION_NOTIFY_EVENT,
    PropMode, StackMode, Window, WindowClass,
};
use x11rb::protocol::xtest::{self, ConnectionExt as _};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use crate::error::InputError;

fn x11_error(err: impl std::fmt::Display) -> InputError {
    InputError::X11Connection(err.to_string())
}

x11rb::atom_manager! {
    Atoms: AtomsCookie {
        XdndAware,
        XdndSelection,
        XdndEnter,
        XdndPosition,
        XdndStatus,
        XdndLeave,
        XdndTypeList,
        URI_LIST: b"text/uri-list",
        ION_DND_DATA,
    }
}

/// Versión de XDND que anunciamos. La 5 es la vigente desde 2002 y la que
/// hablan GTK, Qt y Chromium.
const XDND_VERSION: u32 = 5;

/// Lado de la ventana sonda. Alcanza con que contenga al puntero durante el
/// vaivén de un píxel; se mantiene chica porque sin compositor se ve.
const PROBE_SIZE: u16 = 7;

/// Cuánto se espera a que el origen del arrastre nos salude. Un origen con
/// un arrastre activo responde en el siguiente evento de movimiento — unos
/// pocos milisegundos; este plazo es el costo de un cruce con el botón
/// apretado que **no** era un arrastre.
const ENTER_TIMEOUT: Duration = Duration::from_millis(150);

/// Cuánto se espera a que el origen entregue la lista de archivos.
const DATA_TIMEOUT: Duration = Duration::from_millis(500);

/// Cuánto se espera a que el origen suelte el puntero después de cada
/// intento de cancelación.
const CANCEL_TIMEOUT: Duration = Duration::from_millis(150);

/// Keysym de Escape (`XK_Escape`).
const KEYSYM_ESCAPE: u32 = 0xff1b;

/// Keycode de Escape en cualquier teclado evdev/xfree86, por si el servidor
/// no responde la consulta del mapa de teclado.
const FALLBACK_ESCAPE_KEYCODE: u8 = 9;

/// Lo que devuelve [`take_dragged_files`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DraggedFiles {
    /// Archivos y carpetas que el usuario estaba arrastrando. Vacío si no
    /// había un arrastre de archivos en curso, o si no se lo pudo cancelar.
    pub paths: Vec<PathBuf>,
    /// Keycode **evdev** (el del protocolo, ver `super::capture`) del Escape
    /// que se inyectó para cancelar el arrastre, si se inyectó. Quien
    /// también esté capturando el teclado tiene que descartar esa
    /// pulsación: no la hizo el usuario.
    pub injected_escape: Option<u32>,
}

/// Si hay un arrastre de archivos en curso en este equipo, devuelve los
/// archivos y lo cancela. Bloquea unos cientos de milisegundos como mucho
/// (los plazos de este módulo); sin ningún arrastre reciente vuelve al
/// instante.
///
/// Cualquier falla (sin servidor X, sin XTEST, un origen que no coopera) se
/// trata como "no hay arrastre": esta función nunca debe impedir un cambio
/// de control normal.
#[must_use]
pub fn take_dragged_files() -> DraggedFiles {
    match probe() {
        Ok(found) => found,
        Err(err) => {
            tracing::debug!(%err, "no se pudo consultar el arrastre en curso");
            DraggedFiles::default()
        }
    }
}

fn probe() -> Result<DraggedFiles, InputError> {
    let (conn, screen_num) = x11rb::connect(None).map_err(x11_error)?;
    let root = conn.setup().roots[screen_num].root;
    let atoms = Atoms::new(&conn)
        .map_err(x11_error)?
        .reply()
        .map_err(x11_error)?;

    // Atajo: sin dueño de `XdndSelection` nadie arrastró nada todavía en
    // esta sesión. Es el caso de casi todos los cruces y no cuesta espera.
    let owner = conn
        .get_selection_owner(atoms.XdndSelection)
        .map_err(x11_error)?
        .reply()
        .map_err(x11_error)?
        .owner;
    if owner == x11rb::NONE {
        return Ok(DraggedFiles::default());
    }
    conn.extension_information(xtest::X11_EXTENSION_NAME)
        .map_err(x11_error)?
        .ok_or(InputError::MissingX11Extension("XTEST"))?;

    let window = create_probe_window(&conn, root, atoms)?;
    let result = handshake(&conn, root, window, atoms);
    let _ = conn.destroy_window(window);
    let _ = conn.flush();
    let Some((source, paths)) = result? else {
        return Ok(DraggedFiles::default());
    };

    let keycode = escape_keycode(&conn);
    let cancelled = cancel_drag(&conn, root, source, keycode)?;
    Ok(DraggedFiles {
        paths: if cancelled { paths } else { Vec::new() },
        // X11 = evdev + 8 (offset de XKB), igual que en `super::capture`.
        injected_escape: Some(u32::from(keycode).saturating_sub(8)),
    })
}

/// Termina el arrastre en curso sin que se suelte nada, y confirma que el
/// origen liberó el puntero. `false` si no hubo caso.
///
/// Dos mecanismos, en este orden:
///
/// 1. **Escape.** Es la forma estándar de cancelar: GTK 3, Qt y Chromium
///    tienen el teclado agarrado mientras dura el arrastre y lo atienden.
/// 2. **Romper el agarre del puntero.** GTK 4 no mira el teclado durante un
///    arrastre en X11 (su `gdk_x11_drag_handle_event` solo atiende
///    movimiento, botón soltado y agarre roto), así que Escape no le hace
///    nada. Lo que sí lo cancela es perder su agarre, y el servidor X se lo
///    quita en cuanto la ventana sobre la que lo pidió deja de ser visible.
///    Esa ventana es la misma que figura como origen XDND: un ayudante de
///    1×1 que GTK crea para cada arrastre. Solo se la desmapea si tiene
///    exactamente esa pinta — jamás una ventana real de la aplicación — y
///    se la vuelve a mapear enseguida.
fn cancel_drag(
    conn: &RustConnection,
    root: Window,
    source: Window,
    escape: u8,
) -> Result<bool, InputError> {
    conn.xtest_fake_input(KEY_PRESS_EVENT, escape, x11rb::CURRENT_TIME, root, 0, 0, 0)
        .map_err(x11_error)?;
    conn.xtest_fake_input(KEY_RELEASE_EVENT, escape, x11rb::CURRENT_TIME, root, 0, 0, 0)
        .map_err(x11_error)?;
    conn.flush().map_err(x11_error)?;
    if wait_pointer_free(conn, root)? {
        return Ok(true);
    }

    if !is_helper_window(conn, source) {
        return Ok(false);
    }
    conn.unmap_window(source).map_err(x11_error)?;
    conn.flush().map_err(x11_error)?;
    let released = wait_pointer_free(conn, root)?;
    // Si el toolkit reutiliza el ayudante, lo necesita visible la próxima
    // vez. Si ya lo destruyó, el error de este pedido se ignora.
    let _ = conn.map_window(source);
    let _ = conn.flush();
    Ok(released)
}

/// Espera hasta [`CANCEL_TIMEOUT`] a que nadie tenga agarrado el puntero. La
/// única forma de preguntarlo en X11 es intentar agarrarlo uno mismo.
fn wait_pointer_free(conn: &RustConnection, root: Window) -> Result<bool, InputError> {
    let deadline = Instant::now() + CANCEL_TIMEOUT;
    loop {
        let status = conn
            .grab_pointer(
                false,
                root,
                EventMask::NO_EVENT,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
                x11rb::NONE,
                x11rb::NONE,
                x11rb::CURRENT_TIME,
            )
            .map_err(x11_error)?
            .reply()
            .map_err(x11_error)?
            .status;
        if status == GrabStatus::SUCCESS {
            conn.ungrab_pointer(x11rb::CURRENT_TIME).map_err(x11_error)?;
            conn.flush().map_err(x11_error)?;
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// `true` si `window` es una ventana auxiliar y no una ventana real de la
/// aplicación: de 1×1 (o 2×2) y sin decoración del gestor de ventanas. GTK 3
/// la ubica en (-99, -99) y GTK 4 en (0, 0); lo que las distingue de
/// cualquier ventana que el usuario pueda ver o usar es el tamaño.
fn is_helper_window(conn: &RustConnection, window: Window) -> bool {
    let Some(geometry) = conn.get_geometry(window).ok().and_then(|c| c.reply().ok()) else {
        return false;
    };
    let Some(attributes) = conn
        .get_window_attributes(window)
        .ok()
        .and_then(|c| c.reply().ok())
    else {
        return false;
    };
    geometry.width <= 2 && geometry.height <= 2 && attributes.override_redirect
}

/// Crea, bajo el puntero y por encima de todo, la ventana que hace de
/// destino XDND.
fn create_probe_window(
    conn: &RustConnection,
    root: Window,
    atoms: Atoms,
) -> Result<Window, InputError> {
    let pointer = conn
        .query_pointer(root)
        .map_err(x11_error)?
        .reply()
        .map_err(x11_error)?;
    let half = i16::try_from(PROBE_SIZE / 2).unwrap_or(0);
    let window = conn.generate_id().map_err(x11_error)?;
    conn.create_window(
        x11rb::COPY_DEPTH_FROM_PARENT,
        window,
        root,
        pointer.root_x.saturating_sub(half),
        pointer.root_y.saturating_sub(half),
        PROBE_SIZE,
        PROBE_SIZE,
        0,
        WindowClass::INPUT_OUTPUT,
        x11rb::COPY_FROM_PARENT,
        // `override_redirect`: el gestor de ventanas no la decora, no la
        // mueve ni le da foco. Sin fondo propio para no pintar nada.
        &CreateWindowAux::new()
            .override_redirect(1)
            .background_pixmap(x11rb::NONE),
    )
    .map_err(x11_error)?;
    conn.change_property32(
        PropMode::REPLACE,
        window,
        atoms.XdndAware,
        AtomEnum::ATOM,
        &[XDND_VERSION],
    )
    .map_err(x11_error)?;
    conn.map_window(window).map_err(x11_error)?;
    conn.configure_window(
        window,
        &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
    )
    .map_err(x11_error)?;
    conn.flush().map_err(x11_error)?;
    Ok(window)
}

/// Mueve el puntero un píxel y lo devuelve a su lugar, para que el origen
/// del arrastre reciba un evento de movimiento y vuelva a buscar el destino
/// bajo el cursor. Se prueba en las dos direcciones porque contra un borde
/// una de ellas no mueve nada.
fn nudge_pointer(conn: &RustConnection, root: Window) -> Result<(), InputError> {
    // `detail = 1` → movimiento relativo.
    for (dx, dy) in [(-1, -1), (1, 1), (1, 1), (-1, -1)] {
        conn.xtest_fake_input(MOTION_NOTIFY_EVENT, 1, x11rb::CURRENT_TIME, root, dx, dy, 0)
            .map_err(x11_error)?;
    }
    conn.flush().map_err(x11_error)
}

/// Lado destino de XDND: espera el saludo del origen, comprueba que ofrece
/// archivos y se los pide. Devuelve la ventana origen y la lista, o `None`
/// si no hay un arrastre de archivos en curso.
fn handshake(
    conn: &RustConnection,
    root: Window,
    window: Window,
    atoms: Atoms,
) -> Result<Option<(Window, Vec<PathBuf>)>, InputError> {
    nudge_pointer(conn, root)?;

    let mut source: Option<Window> = None;
    let mut requested = false;
    let mut deadline = Instant::now() + ENTER_TIMEOUT;
    loop {
        let Some(event) = conn.poll_for_event().map_err(x11_error)? else {
            if Instant::now() >= deadline {
                return Ok(None);
            }
            std::thread::sleep(Duration::from_millis(2));
            continue;
        };
        match event {
            Event::ClientMessage(message) if message.type_ == atoms.XdndEnter => {
                let data = message.data.as_data32();
                if !offers_uri_list(conn, atoms, &data)? {
                    // Arrastre de otra cosa (texto, una pestaña...): no es
                    // nuestro asunto, se lo deja seguir.
                    return Ok(None);
                }
                source = Some(data[0]);
            }
            Event::ClientMessage(message) if message.type_ == atoms.XdndPosition => {
                let data = message.data.as_data32();
                let Some(source) = source.filter(|source| *source == data[0]) else {
                    continue;
                };
                // Cortesía de protocolo: el origen espera un `XdndStatus`
                // por cada `XdndPosition`. Bit 0 en cero = "acá no se suelta".
                let status = ClientMessageEvent::new(
                    32,
                    source,
                    atoms.XdndStatus,
                    [window, 0, 0, 0, x11rb::NONE],
                );
                conn.send_event(false, source, EventMask::NO_EVENT, status)
                    .map_err(x11_error)?;
                if !requested {
                    requested = true;
                    // El timestamp del `XdndPosition` es el que el origen
                    // espera ver en el pedido de datos.
                    conn.convert_selection(
                        window,
                        atoms.XdndSelection,
                        atoms.URI_LIST,
                        atoms.ION_DND_DATA,
                        data[3],
                    )
                    .map_err(x11_error)?;
                    deadline = Instant::now() + DATA_TIMEOUT;
                }
                conn.flush().map_err(x11_error)?;
            }
            Event::SelectionNotify(notify) if notify.requestor == window => {
                if notify.property == x11rb::NONE {
                    return Ok(None);
                }
                let reply = conn
                    .get_property(
                        true,
                        window,
                        atoms.ION_DND_DATA,
                        AtomEnum::ANY,
                        0,
                        u32::MAX,
                    )
                    .map_err(x11_error)?
                    .reply()
                    .map_err(x11_error)?;
                let paths = paths_from_uri_list(&String::from_utf8_lossy(&reply.value));
                return Ok(source
                    .filter(|_| !paths.is_empty())
                    .map(|source| (source, paths)));
            }
            _ => {}
        }
    }
}

/// `true` si el origen del arrastre ofrece `text/uri-list`. Hasta tres
/// tipos viajan dentro del propio `XdndEnter`; con más, están en la
/// propiedad `XdndTypeList` de la ventana origen.
fn offers_uri_list(
    conn: &RustConnection,
    atoms: Atoms,
    enter: &[u32; 5],
) -> Result<bool, InputError> {
    let more_than_three = enter[1] & 1 != 0;
    if !more_than_three {
        return Ok(enter[2..5].contains(&atoms.URI_LIST));
    }
    let reply = conn
        .get_property(false, enter[0], atoms.XdndTypeList, AtomEnum::ATOM, 0, 1024)
        .map_err(x11_error)?
        .reply()
        .map_err(x11_error)?;
    Ok(reply
        .value32()
        .is_some_and(|mut types| types.any(|atom: Atom| atom == atoms.URI_LIST)))
}

/// Keycode que hoy produce Escape en este servidor X.
fn escape_keycode(conn: &RustConnection) -> u8 {
    let setup = conn.setup();
    let (min, max) = (setup.min_keycode, setup.max_keycode);
    let Some(mapping) = conn
        .get_keyboard_mapping(min, max.saturating_sub(min).saturating_add(1))
        .ok()
        .and_then(|cookie| cookie.reply().ok())
    else {
        return FALLBACK_ESCAPE_KEYCODE;
    };
    let per_keycode = usize::from(mapping.keysyms_per_keycode).max(1);
    mapping
        .keysyms
        .chunks(per_keycode)
        .position(|keysyms| keysyms.first() == Some(&KEYSYM_ESCAPE))
        .and_then(|index| u8::try_from(index).ok())
        .map_or(FALLBACK_ESCAPE_KEYCODE, |index| min.saturating_add(index))
}
