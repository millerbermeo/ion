use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{
    ConfigureWindowAux, ConnectionExt as _, CreateWindowAux, GrabMode, GrabStatus, Window,
    WindowClass,
};
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
