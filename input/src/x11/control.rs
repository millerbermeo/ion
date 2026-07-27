use std::time::Duration;

use x11rb::connection::Connection as _;
use x11rb::protocol::xkb::ConnectionExt as _;
use x11rb::protocol::xproto::{
    AutoRepeatMode, ConfigureWindowAux, ConnectionExt as _, CreateWindowAux, GrabMode, GrabStatus,
    Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;

use crate::error::InputError;

fn x11_error(err: impl std::fmt::Display) -> InputError {
    InputError::X11Connection(err.to_string())
}

/// Identificador del teclado core en el espacio de `DeviceSpec` de XKB
/// (`XkbUseCoreKbd` de la especificación) — no hay constante nombrada en
/// `x11rb`.
const XKB_USE_CORE_KBD: u16 = 0x0100;

/// Cuántos keycodes describe el bitmask `auto_repeats` del protocolo core
/// (32 bytes × 8 bits = los 256 keycodes posibles de X11).
const AUTO_REPEAT_BITMASK_LEN: usize = 32;

/// Configuración de auto-repetición de teclas del servidor X local: cuánto
/// hay que mantener una tecla antes de que empiece a repetirse, cada cuánto
/// se repite después, y qué teclas participan.
///
/// Hace falta porque los eventos `XI_RawKeyPress` que usa
/// [`super::X11Capture`] **no** incluyen las repeticiones que el servidor X
/// genera al mantener una tecla (medido contra un servidor real: un único
/// evento crudo frente a 22 eventos cocidos en 1,5 s). Quien capture tiene
/// que sintetizarlas a partir del estado de teclas mantenidas, y para que
/// se sientan igual que en el teclado local necesita estos tres valores tal
/// como los tiene configurados el usuario.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyRepeatSettings {
    /// Cuánto hay que mantener la tecla antes de la primera repetición.
    pub delay: Duration,
    /// Tiempo entre repeticiones una vez arrancadas.
    pub interval: Duration,
    /// `false` si el usuario desactivó la repetición para todo el teclado
    /// (`xset -r`) — en ese caso no hay que sintetizar nada.
    pub enabled: bool,
    /// Bit por keycode **X11** (no evdev): 1 = esa tecla se repite. Los
    /// modificadores (Shift, Ctrl, Alt, ...) vienen en 0, que es justo lo
    /// que evita que se repitan cuando alguien los mantiene apretados.
    per_key: [u8; AUTO_REPEAT_BITMASK_LEN],
}

impl Default for KeyRepeatSettings {
    /// Valores por defecto de X11 (delay 500 ms, ~33 repeticiones/s) con
    /// todas las teclas habilitadas — lo que se usa si el servidor no
    /// responde la consulta, para que la repetición siga funcionando en vez
    /// de desaparecer.
    fn default() -> Self {
        Self {
            delay: Duration::from_millis(500),
            interval: Duration::from_millis(30),
            enabled: true,
            per_key: [0xff; AUTO_REPEAT_BITMASK_LEN],
        }
    }
}

impl KeyRepeatSettings {
    /// `true` si esta tecla debe repetirse al mantenerse. `keycode` va en el
    /// espacio **evdev** (el que viaja por el protocolo, ver
    /// `super::capture`), así que acá se le vuelve a sumar el offset de 8 de
    /// XKB para consultar el bitmask.
    #[must_use]
    pub fn repeats(&self, keycode: u32) -> bool {
        if !self.enabled {
            return false;
        }
        let x11_keycode = keycode.saturating_add(8) as usize;
        let (byte, bit) = (x11_keycode / 8, x11_keycode % 8);
        self.per_key
            .get(byte)
            .is_some_and(|bits| bits & (1 << bit) != 0)
    }
}

/// Agarrar/soltar el puntero+teclado y mover el cursor real del sistema
/// operativo — operaciones de control que se hacen sobre una conexión X11
/// **separada** de la de [`super::X11Capture`].
///
/// No pueden compartir conexión: `X11Capture::run` bloquea su hilo entero
/// dentro de `wait_for_event` sobre su propia conexión mientras dure la
/// captura, así que cualquier otro hilo que necesite pedir un grab o un
/// warp *al mismo tiempo* necesita su propio socket al servidor X (esto es
/// normal y soportado — un mismo cliente puede abrir tantas conexiones
/// como quiera).
pub struct X11Control {
    conn: RustConnection,
    root: Window,
    /// Ventana `InputOnly` de 1x1 invisible, reposicionada sobre el punto
    /// de entrada y usada como `confine_to` en cada [`Self::grab`] — sin
    /// ella, `grab_pointer` con modo `ASYNC` y `confine_to = None` no
    /// confina nada: el ícono real sigue el mouse físico libremente por
    /// toda la pantalla mientras el control ya pasó al remoto, dando la
    /// sensación de que el movimiento se "duplica" en ambos equipos.
    /// Confinar el cursor a esta ventana lo deja clavado en el punto de
    /// hand-off mientras dure el grab, sin afectar los deltas crudos de
    /// `XI_RawMotion` (que reflejan el hardware, no la posición on-screen).
    confine_window: Window,
    /// Tamaño real de la pantalla raíz — para acotar a [`Self::grab`] la
    /// posición donde se planta `confine_window`. El `(x, y)` que dispara
    /// un hand-off puede caer justo un poco más allá del borde físico
    /// (es lo que hace que se detecte el cruce, ver `HandoffState`), así
    /// que sin este clamp la ventana de confinamiento terminaría fuera de
    /// la pantalla real.
    width: u16,
    height: u16,
}

impl X11Control {
    /// # Errors
    ///
    /// Devuelve [`InputError::X11Connection`] si no hay servidor X
    /// disponible.
    pub fn connect() -> Result<Self, InputError> {
        let (conn, screen_num) = x11rb::connect(None).map_err(x11_error)?;
        let screen = &conn.setup().roots[screen_num];
        let root = screen.root;
        let width = screen.width_in_pixels;
        let height = screen.height_in_pixels;

        let confine_window = conn.generate_id().map_err(x11_error)?;
        conn.create_window(
            0,
            confine_window,
            root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_ONLY,
            0,
            &CreateWindowAux::new().override_redirect(1),
        )
        .map_err(x11_error)?
        .check()
        .map_err(x11_error)?;

        Ok(Self {
            conn,
            root,
            confine_window,
            width,
            height,
        })
    }

    /// Ancho/alto en píxeles de la pantalla raíz — en un `Xorg` con varios
    /// monitores lado a lado (el caso normal en Linux, a diferencia de
    /// Windows/macOS) esto ya es el escritorio virtual combinado completo,
    /// sin necesitar la extensión `RandR`. Quien llama es responsable de
    /// posicionarlo dentro de un [`ionconnect_screen::MonitorGeometry`]
    /// (este crate no depende de `ionconnect-screen` a propósito).
    ///
    /// # Errors
    ///
    /// Devuelve [`InputError::X11Connection`] si no hay servidor X
    /// disponible.
    pub fn root_geometry() -> Result<(u32, u32), InputError> {
        let (conn, screen_num) = x11rb::connect(None).map_err(x11_error)?;
        let screen = &conn.setup().roots[screen_num];
        Ok((
            u32::from(screen.width_in_pixels),
            u32::from(screen.height_in_pixels),
        ))
    }

    /// Configuración de auto-repetición del teclado tal como la tiene el
    /// usuario en este equipo (lo que muestra `xset q`). Nunca falla hacia
    /// afuera: si el servidor no soporta XKB o rechaza alguna consulta, cae
    /// a [`KeyRepeatSettings::default`] para que la repetición siga
    /// funcionando con los valores estándar de X11 en vez de desaparecer.
    #[must_use]
    pub fn key_repeat_settings(&self) -> KeyRepeatSettings {
        let mut settings = KeyRepeatSettings::default();

        // `auto_repeats` y el interruptor global vienen del protocolo core,
        // sin necesidad de ninguna extensión.
        if let Ok(cookie) = self.conn.get_keyboard_control()
            && let Ok(reply) = cookie.reply()
        {
            settings.per_key = reply.auto_repeats;
            settings.enabled = reply.global_auto_repeat == AutoRepeatMode::ON;
        }

        // El delay y el intervalo, en cambio, solo existen en XKB. Hay que
        // negociar la extensión en esta conexión antes de consultarlos.
        if self.conn.xkb_use_extension(1, 0).is_ok_and(|cookie| {
            cookie.reply().is_ok_and(|reply| reply.supported)
        }) && let Ok(cookie) = self.conn.xkb_get_controls(XKB_USE_CORE_KBD)
            && let Ok(reply) = cookie.reply()
        {
            // Un servidor puede reportar 0 (o valores absurdamente chicos);
            // sin este piso, sintetizar repeticiones a ese ritmo inundaría
            // la red y al equipo remoto.
            settings.delay = Duration::from_millis(u64::from(reply.repeat_delay).max(50));
            settings.interval = Duration::from_millis(u64::from(reply.repeat_interval).max(10));
        }

        settings
    }

    /// Agarra el puntero y el teclado exclusivamente para este cliente: a
    /// partir de este punto el resto del sistema deja de recibir eventos de
    /// entrada normales — es lo que hay que llamar justo al detectar un
    /// hand-off hacia un equipo remoto.
    ///
    /// Confina el cursor real a `(x, y)` (el punto de hand-off) vía una
    /// ventana `InputOnly` de 1x1 invisible — sin esto el ícono sigue el
    /// mouse físico libremente por toda la pantalla local aunque el
    /// control ya haya pasado al remoto, dando la sensación de que el
    /// movimiento se duplica en ambos equipos. `cursor` en `0` (`XCB_NONE`)
    /// significa "no cambiar el ícono del cursor".
    ///
    /// # Errors
    ///
    /// Devuelve [`InputError::X11Connection`] si el servidor rechaza el
    /// grab (por ejemplo, si otro cliente ya lo tiene).
    #[allow(clippy::cast_possible_truncation)]
    pub fn grab(&self, x: i32, y: i32) -> Result<(), InputError> {
        let x = x.clamp(0, i32::from(self.width) - 1);
        let y = y.clamp(0, i32::from(self.height) - 1);
        self.conn
            .configure_window(
                self.confine_window,
                &ConfigureWindowAux::new().x(x).y(y),
            )
            .map_err(x11_error)?;
        self.conn
            .map_window(self.confine_window)
            .map_err(x11_error)?;

        let reply = self
            .conn
            .grab_pointer(
                false,
                self.root,
                x11rb::protocol::xproto::EventMask::NO_EVENT,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
                self.confine_window,
                0u32,
                0u32,
            )
            .map_err(x11_error)?
            .reply()
            .map_err(x11_error)?;
        if reply.status != GrabStatus::SUCCESS {
            let _ = self.conn.unmap_window(self.confine_window);
            self.conn.flush().map_err(x11_error)?;
            return Err(InputError::X11Connection(format!(
                "grab_pointer falló con status {:?}",
                reply.status
            )));
        }
        self.conn
            .grab_keyboard(false, self.root, 0u32, GrabMode::ASYNC, GrabMode::ASYNC)
            .map_err(x11_error)?
            .reply()
            .map_err(x11_error)?;
        self.conn.flush().map_err(x11_error)?;
        Ok(())
    }

    /// Libera el agarre exclusivo de puntero y teclado, devolviendo el
    /// control normal al resto del sistema — y desmapea la ventana de
    /// confinamiento usada por [`Self::grab`].
    ///
    /// # Errors
    ///
    /// Devuelve [`InputError::X11Connection`] si falla la solicitud.
    pub fn ungrab(&self) -> Result<(), InputError> {
        self.conn.ungrab_pointer(0u32).map_err(x11_error)?;
        self.conn.ungrab_keyboard(0u32).map_err(x11_error)?;
        self.conn
            .unmap_window(self.confine_window)
            .map_err(x11_error)?;
        self.conn.flush().map_err(x11_error)?;
        Ok(())
    }

    /// Mueve el cursor real del sistema operativo a `(x, y)` — llamar al
    /// devolver el control a este equipo, para que el cursor reaparezca en
    /// el punto de entrada correcto en vez de quedar donde lo dejó el
    /// último `grab`.
    ///
    /// # Errors
    ///
    /// Devuelve [`InputError::X11Connection`] si falla la solicitud.
    #[allow(clippy::cast_possible_truncation)]
    pub fn warp_to(&self, x: i32, y: i32) -> Result<(), InputError> {
        self.conn
            .warp_pointer(0u32, self.root, 0, 0, 0, 0, x as i16, y as i16)
            .map_err(x11_error)?;
        self.conn.flush().map_err(x11_error)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bitmask real de un Ubuntu con la configuración de fábrica, tal como
    /// lo reporta `xset q` (`auto repeating keys: 00ffffffdffffbbf...`).
    fn settings() -> KeyRepeatSettings {
        let mut per_key = [0xffu8; AUTO_REPEAT_BITMASK_LEN];
        per_key[0] = 0x00;
        per_key[4] = 0xdf;
        per_key[6] = 0xfb;
        per_key[7] = 0xbf;
        KeyRepeatSettings {
            per_key,
            ..KeyRepeatSettings::default()
        }
    }

    #[test]
    fn ordinary_keys_repeat() {
        // evdev 30 = 'a' (keycode X11 38), evdev 14 = Backspace (X11 22),
        // evdev 105 = flecha izquierda (X11 113).
        for keycode in [30, 14, 105] {
            assert!(
                settings().repeats(keycode),
                "la tecla evdev {keycode} debería repetirse"
            );
        }
    }

    #[test]
    fn modifiers_do_not_repeat() {
        // evdev 42 = Shift_L (X11 50), evdev 29 = Control_L (X11 37),
        // evdev 54 = Shift_R (X11 62).
        for keycode in [42, 29, 54] {
            assert!(
                !settings().repeats(keycode),
                "el modificador evdev {keycode} no debería repetirse"
            );
        }
    }

    #[test]
    fn nothing_repeats_when_globally_disabled() {
        let disabled = KeyRepeatSettings {
            enabled: false,
            ..settings()
        };
        assert!(!disabled.repeats(30));
    }

    #[test]
    fn keycodes_past_the_bitmask_do_not_repeat() {
        // 256 - 8 = primer keycode evdev que ya no entra en los 32 bytes.
        assert!(!settings().repeats(248));
        assert!(!settings().repeats(u32::MAX));
    }

    #[test]
    fn defaults_repeat_every_key() {
        let defaults = KeyRepeatSettings::default();
        assert!(defaults.repeats(30));
        assert!(defaults.enabled);
    }
}
