use std::collections::HashSet;

use x11rb::connection::{Connection as _, RequestConnection as _};
use x11rb::protocol::xproto::{
    BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT, KEY_PRESS_EVENT, KEY_RELEASE_EVENT,
    MOTION_NOTIFY_EVENT, Window,
};
use x11rb::protocol::xtest::{self, ConnectionExt as _};
use x11rb::rust_connection::RustConnection;

use crate::error::InputError;
use crate::event::CapturedEvent;
use crate::inject::InputInjector;
use crate::x11::util::button_to_code;

fn x11_error(err: impl std::fmt::Display) -> InputError {
    InputError::X11Connection(err.to_string())
}

/// Inyecta eventos vía la extensión XTEST. No confirma cada request con
/// `.check()` (eso obligaría a un round-trip por movimiento de mouse,
/// disparando la latencia); en cambio, los errores del servidor se
/// detectarían en el siguiente request que sí se confirme.
pub struct X11Injector {
    conn: RustConnection,
    root: Window,
    /// Teclas que este inyector marcó como presionadas y todavía no soltó,
    /// para reconocer una repetición (una pulsación de algo que ya está
    /// mantenido) — ver [`X11Injector::inject`] para qué se hace distinto
    /// en ese caso.
    held_keys: HashSet<u8>,
}

impl X11Injector {
    /// # Errors
    ///
    /// Devuelve [`InputError::X11Connection`] si no hay servidor X
    /// disponible, o [`InputError::MissingX11Extension`] si el servidor no
    /// soporta XTEST.
    pub fn connect() -> Result<Self, InputError> {
        let (conn, screen_num) = x11rb::connect(None).map_err(x11_error)?;
        let root = conn.setup().roots[screen_num].root;
        conn.extension_information(xtest::X11_EXTENSION_NAME)
            .map_err(x11_error)?
            .ok_or(InputError::MissingX11Extension("XTEST"))?;
        Ok(Self {
            conn,
            root,
            held_keys: HashSet::new(),
        })
    }
}

impl InputInjector for X11Injector {
    fn inject(&mut self, event: &CapturedEvent) -> Result<(), InputError> {
        match *event {
            // Solo tiene sentido del lado de captura (detección de borde
            // antes de agarrar el puntero); no hay nada que inyectar.
            CapturedEvent::AbsolutePosition { .. } => {}
            CapturedEvent::MouseMove { x, y } => {
                let (x, y) = (clamp_to_i16(x), clamp_to_i16(y));
                self.conn
                    .xtest_fake_input(MOTION_NOTIFY_EVENT, 0, 0, self.root, x, y, 0)
                    .map_err(x11_error)?;
            }
            CapturedEvent::MouseButton { button, pressed } => {
                let event_type = if pressed {
                    BUTTON_PRESS_EVENT
                } else {
                    BUTTON_RELEASE_EVENT
                };
                self.conn
                    .xtest_fake_input(event_type, button_to_code(button), 0, self.root, 0, 0, 0)
                    .map_err(x11_error)?;
            }
            CapturedEvent::Key {
                keycode, pressed, ..
            } => {
                // `keycode` viaja como keycode `evdev` (ver
                // `x11::capture::x11_keycode_to_evdev`) — hay que sumarle
                // de vuelta el offset de 8 que usa XKB antes de mandarlo a
                // `xtest_fake_input`, que espera keycodes X11 nativos.
                let code = u8::try_from(keycode.saturating_add(8)).map_err(|_| {
                    InputError::X11Connection(format!("keycode fuera de rango: {keycode}"))
                })?;
                if pressed {
                    // Una pulsación de algo que ya está mantenido es una
                    // repetición sintetizada por el emisor (ver
                    // `core::key_repeat`). No alcanza con reenviarla tal
                    // cual: una tecla mantenida vía XTEST hace que *este*
                    // servidor X arranque además su propio auto-repeat
                    // (medido: 22 repeticiones en 1,5 s a partir de una sola
                    // pulsación XTEST), así que sumar las nuestras encima
                    // daría el doble de velocidad. Mandar release+press
                    // entrega exactamente una repetición a las aplicaciones
                    // y de paso reinicia el temporizador de auto-repeat
                    // local, que por eso nunca llega a dispararse mientras
                    // las repeticiones remotas sigan llegando.
                    if !self.held_keys.insert(code) {
                        self.conn
                            .xtest_fake_input(KEY_RELEASE_EVENT, code, 0, self.root, 0, 0, 0)
                            .map_err(x11_error)?;
                    }
                    self.conn
                        .xtest_fake_input(KEY_PRESS_EVENT, code, 0, self.root, 0, 0, 0)
                        .map_err(x11_error)?;
                } else {
                    self.held_keys.remove(&code);
                    self.conn
                        .xtest_fake_input(KEY_RELEASE_EVENT, code, 0, self.root, 0, 0, 0)
                        .map_err(x11_error)?;
                }
            }
        }
        self.conn.flush().map_err(x11_error)?;
        Ok(())
    }
}

#[allow(clippy::cast_possible_truncation)]
fn clamp_to_i16(value: i32) -> i16 {
    value.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}
