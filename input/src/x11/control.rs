use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{ConnectionExt as _, GrabMode, GrabStatus, Window};
use x11rb::rust_connection::RustConnection;

use crate::error::InputError;

fn x11_error(err: impl std::fmt::Display) -> InputError {
    InputError::X11Connection(err.to_string())
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
}

impl X11Control {
    /// # Errors
    ///
    /// Devuelve [`InputError::X11Connection`] si no hay servidor X
    /// disponible.
    pub fn connect() -> Result<Self, InputError> {
        let (conn, screen_num) = x11rb::connect(None).map_err(x11_error)?;
        let root = conn.setup().roots[screen_num].root;
        Ok(Self { conn, root })
    }

    /// Agarra el puntero y el teclado exclusivamente para este cliente: a
    /// partir de este punto el resto del sistema deja de recibir eventos de
    /// entrada normales — es lo que hay que llamar justo al detectar un
    /// hand-off hacia un equipo remoto.
    ///
    /// No oculta el cursor visualmente (dejar el ícono real quieto en el
    /// borde es un defecto cosmético conocido, no funcional). `confine_to`/
    /// `cursor` en `0` (`XCB_NONE`) significan "sin restricción de ventana"
    /// / "no cambiar el ícono del cursor".
    ///
    /// # Errors
    ///
    /// Devuelve [`InputError::X11Connection`] si el servidor rechaza el
    /// grab (por ejemplo, si otro cliente ya lo tiene).
    pub fn grab(&self) -> Result<(), InputError> {
        let reply = self
            .conn
            .grab_pointer(
                false,
                self.root,
                x11rb::protocol::xproto::EventMask::NO_EVENT,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
                0u32,
                0u32,
                0u32,
            )
            .map_err(x11_error)?
            .reply()
            .map_err(x11_error)?;
        if reply.status != GrabStatus::SUCCESS {
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
    /// control normal al resto del sistema.
    ///
    /// # Errors
    ///
    /// Devuelve [`InputError::X11Connection`] si falla la solicitud.
    pub fn ungrab(&self) -> Result<(), InputError> {
        self.conn.ungrab_pointer(0u32).map_err(x11_error)?;
        self.conn.ungrab_keyboard(0u32).map_err(x11_error)?;
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
