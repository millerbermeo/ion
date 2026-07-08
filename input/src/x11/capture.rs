use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

use ionconnect_shared::KeyModifiers;
use x11rb::connection::Connection as _;
use x11rb::protocol::Event;
use x11rb::protocol::xinput::{ConnectionExt as _, EventMask, XIEventMask};
use x11rb::protocol::xproto::Window;
use x11rb::rust_connection::RustConnection;

use crate::capture::InputCapture;
use crate::error::InputError;
use crate::event::CapturedEvent;
use crate::x11::util::{button_from_code, valuator_value};

fn x11_error(err: impl std::fmt::Display) -> InputError {
    InputError::X11Connection(err.to_string())
}

/// El protocolo XI2 identifica "todos los dispositivos" con el id 0 (no hay
/// una constante nombrada para esto en `x11rb`; es un valor fijo de la
/// especificación `XInput2`, sección 4).
const XI_ALL_DEVICES: u16 = 0;

/// Captura eventos globales de mouse/teclado vía eventos **crudos** de
/// `XInput2` (`XI_RawMotion`/`XI_RawButtonPress`/`XI_RawKeyPress`, etc.).
///
/// Se usan eventos crudos (deltas relativos) en vez de eventos normales
/// (posición absoluta) porque los crudos se entregan sin importar qué
/// ventana tiene el foco o si el puntero está agarrado/oculto — justo lo que
/// hace falta cuando este equipo está "cediendo" el control y el cursor
/// local ya no importa, solo el movimiento relativo que hay que reenviar.
pub struct X11Capture {
    conn: RustConnection,
    origin_x: i32,
    origin_y: i32,
    stop_flag: Arc<AtomicBool>,
}

impl X11Capture {
    /// `origin_{x,y}` es la posición absoluta desde la que empezar a
    /// acumular deltas (típicamente el punto donde el cursor cruzó el borde
    /// de pantalla hacia este equipo).
    ///
    /// # Errors
    ///
    /// Devuelve [`InputError::X11Connection`] si no hay servidor X
    /// disponible o si la extensión `XInput2` no responde a la versión
    /// solicitada.
    pub fn connect(origin_x: i32, origin_y: i32) -> Result<Self, InputError> {
        let (conn, screen_num) = x11rb::connect(None).map_err(x11_error)?;
        let root: Window = conn.setup().roots[screen_num].root;

        conn.xinput_xi_query_version(2, 2)
            .map_err(x11_error)?
            .reply()
            .map_err(x11_error)?;

        let mask = XIEventMask::RAW_MOTION
            | XIEventMask::RAW_BUTTON_PRESS
            | XIEventMask::RAW_BUTTON_RELEASE
            | XIEventMask::RAW_KEY_PRESS
            | XIEventMask::RAW_KEY_RELEASE;
        let events = [EventMask {
            deviceid: XI_ALL_DEVICES,
            mask: vec![mask],
        }];
        conn.xinput_xi_select_events(root, &events)
            .map_err(x11_error)?
            .check()
            .map_err(x11_error)?;

        Ok(Self {
            conn,
            origin_x,
            origin_y,
            stop_flag: Arc::new(AtomicBool::new(false)),
        })
    }
}

impl InputCapture for X11Capture {
    fn run(&mut self, sink: Sender<CapturedEvent>) -> Result<(), InputError> {
        let mut x = self.origin_x;
        let mut y = self.origin_y;

        while !self.stop_flag.load(Ordering::Relaxed) {
            let event = self.conn.wait_for_event().map_err(x11_error)?;
            let emitted = match event {
                Event::XinputRawMotion(ev) => {
                    let dx = valuator_value(&ev.valuator_mask, &ev.axisvalues, 0).unwrap_or(0.0);
                    let dy = valuator_value(&ev.valuator_mask, &ev.axisvalues, 1).unwrap_or(0.0);
                    x += clamp_delta_to_i32(dx);
                    y += clamp_delta_to_i32(dy);
                    Some(CapturedEvent::MouseMove { x, y })
                }
                Event::XinputRawButtonPress(ev) => {
                    button_from_code(ev.detail).map(|button| CapturedEvent::MouseButton {
                        button,
                        pressed: true,
                    })
                }
                Event::XinputRawButtonRelease(ev) => {
                    button_from_code(ev.detail).map(|button| CapturedEvent::MouseButton {
                        button,
                        pressed: false,
                    })
                }
                Event::XinputRawKeyPress(ev) => Some(CapturedEvent::Key {
                    keycode: ev.detail,
                    modifiers: KeyModifiers::NONE,
                    pressed: true,
                }),
                Event::XinputRawKeyRelease(ev) => Some(CapturedEvent::Key {
                    keycode: ev.detail,
                    modifiers: KeyModifiers::NONE,
                    pressed: false,
                }),
                _ => None,
            };

            if let Some(captured) = emitted
                && sink.send(captured).is_err()
            {
                break;
            }
        }
        Ok(())
    }

    fn stop(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        // `wait_for_event` sigue bloqueado hasta el próximo evento real del
        // servidor; no hay forma de interrumpirlo sin generar uno. Quien
        // orqueste la captura (fase `core`) debe tolerar ese último evento
        // de cola antes de que el hilo termine.
    }
}

/// Trunca un delta de valuador XI2 a un rango razonable antes de sumarlo a
/// la posición acumulada; un solo evento de mouse nunca debería mover miles
/// de píxeles, así que saturar aquí es más seguro que un `as i32` silencioso.
#[allow(clippy::cast_possible_truncation)]
fn clamp_delta_to_i32(value: f64) -> i32 {
    value
        .round()
        .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
}
