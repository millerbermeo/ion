use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

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

/// El protocolo XI2 identifica "todos los dispositivos" (maestros +
/// esclavos) con el id 0, y "todos los maestros" con el id 1 — no hay
/// constantes nombradas para esto en `x11rb`; son valores fijos de la
/// especificación `XInput2`, sección 4.
const XI_ALL_DEVICES: u16 = 0;
const XI_ALL_MASTER_DEVICES: u16 = 1;

/// Posición acumulada compartida entre el hilo de captura (que la actualiza
/// a partir de deltas crudos) y quien orquesta la sesión (que la reinicia
/// al valor exacto del punto de entrada en cada hand-off). Un
/// `Mutex<(i32, i32)>` alcanza: la tasa de eventos de mouse (cientos/s) no
/// genera contención real en un lock que se mantiene microsegundos.
#[derive(Clone)]
pub struct SharedPosition(Arc<Mutex<(i32, i32)>>);

impl SharedPosition {
    #[must_use]
    pub fn new(x: i32, y: i32) -> Self {
        Self(Arc::new(Mutex::new((x, y))))
    }

    /// Reinicia la posición acumulada — llamar exactamente en el momento de
    /// un hand-off, con el punto de entrada calculado por
    /// `screen::Layout::detect_crossing`.
    ///
    /// # Panics
    ///
    /// Solo si el lock quedó envenenado por un panic previo mientras
    /// estaba tomado — no ocurre en uso normal.
    pub fn reset(&self, x: i32, y: i32) {
        *self
            .0
            .lock()
            .expect("el lock de posición no debería estar envenenado") = (x, y);
    }

    /// # Panics
    ///
    /// Solo si el lock quedó envenenado por un panic previo mientras
    /// estaba tomado — no ocurre en uso normal.
    #[must_use]
    pub fn get(&self) -> (i32, i32) {
        *self
            .0
            .lock()
            .expect("el lock de posición no debería estar envenenado")
    }

    fn add(&self, dx: i32, dy: i32) -> (i32, i32) {
        let mut guard = self
            .0
            .lock()
            .expect("el lock de posición no debería estar envenenado");
        guard.0 += dx;
        guard.1 += dy;
        *guard
    }
}

/// Captura eventos globales de mouse/teclado vía `XInput2`.
///
/// Selecciona **ambos** tipos de evento de movimiento y por eso emite dos
/// variantes distintas de [`CapturedEvent`]:
///
/// - `XI_Motion` (no crudo): posición absoluta real del cursor del sistema
///   operativo. Se sigue entregando aunque no tengamos el puntero agarrado,
///   así que sirve para detectar que el cursor llegó al borde de la
///   pantalla mientras el usuario todavía lo controla normalmente →
///   [`CapturedEvent::AbsolutePosition`].
/// - `XI_RawMotion` (crudo): delta relativo de hardware, entregado sin
///   importar grabs ni foco de ventana — por eso sigue funcionando incluso
///   con el puntero agarrado y oculto en el borde, que es exactamente la
///   situación tras un hand-off → [`CapturedEvent::MouseMove`] (acumulado
///   sobre [`SharedPosition`]).
pub struct X11Capture {
    conn: RustConnection,
    position: SharedPosition,
    stop_flag: Arc<AtomicBool>,
}

impl X11Capture {
    /// # Errors
    ///
    /// Devuelve [`InputError::X11Connection`] si no hay servidor X
    /// disponible o si la extensión `XInput2` no responde a la versión
    /// solicitada.
    pub fn connect(position: SharedPosition) -> Result<Self, InputError> {
        let (conn, screen_num) = x11rb::connect(None).map_err(x11_error)?;
        let root: Window = conn.setup().roots[screen_num].root;

        conn.xinput_xi_query_version(2, 2)
            .map_err(x11_error)?
            .reply()
            .map_err(x11_error)?;

        // Dos selecciones separadas, a propósito — mezclarlas en una sola
        // con `deviceid: XI_ALL_DEVICES` duplica cada tecla/clic (probado
        // con `xinput test-xi2`: una pulsación entrega un `KeyPress` desde
        // el dispositivo *maestro* Y otro desde el *esclavo* que lo generó,
        // porque seleccionar sobre "todos los dispositivos" para eventos
        // cocidos se registra en maestro y esclavo a la vez):
        //
        // - Cocidos (`MOTION`/`KEY_PRESS`/`KEY_RELEASE`/`BUTTON_PRESS`/
        //   `BUTTON_RELEASE`) solo en `XI_ALL_MASTER_DEVICES`: el maestro ya
        //   entrega un único evento por pulsación/clic real sin importar
        //   cuántos esclavos físicos tenga detrás.
        // - `RAW_MOTION` solo lo generan los esclavos (los maestros no
        //   tienen eventos crudos propios), así que se queda en
        //   `XI_ALL_DEVICES` — necesario para seguir viendo deltas más allá
        //   del borde una vez agarrado y confinado el puntero (ver
        //   documentación de `X11Capture` arriba).
        let cooked_mask = XIEventMask::MOTION
            | XIEventMask::BUTTON_PRESS
            | XIEventMask::BUTTON_RELEASE
            | XIEventMask::KEY_PRESS
            | XIEventMask::KEY_RELEASE;
        let events = [
            EventMask {
                deviceid: XI_ALL_MASTER_DEVICES,
                mask: vec![cooked_mask],
            },
            EventMask {
                deviceid: XI_ALL_DEVICES,
                mask: vec![XIEventMask::RAW_MOTION],
            },
        ];
        conn.xinput_xi_select_events(root, &events)
            .map_err(x11_error)?
            .check()
            .map_err(x11_error)?;

        Ok(Self {
            conn,
            position,
            stop_flag: Arc::new(AtomicBool::new(false)),
        })
    }

    #[must_use]
    pub fn shared_position(&self) -> SharedPosition {
        self.position.clone()
    }
}

impl InputCapture for X11Capture {
    fn run(&mut self, sink: Sender<CapturedEvent>) -> Result<(), InputError> {
        while !self.stop_flag.load(Ordering::Relaxed) {
            let event = self.conn.wait_for_event().map_err(x11_error)?;
            let emitted = match event {
                Event::XinputMotion(ev) => Some(CapturedEvent::AbsolutePosition {
                    x: ev.root_x >> 16,
                    y: ev.root_y >> 16,
                }),
                Event::XinputRawMotion(ev) => {
                    let dx = valuator_value(&ev.valuator_mask, &ev.axisvalues, 0).unwrap_or(0.0);
                    let dy = valuator_value(&ev.valuator_mask, &ev.axisvalues, 1).unwrap_or(0.0);
                    let (x, y) = self
                        .position
                        .add(clamp_delta_to_i32(dx), clamp_delta_to_i32(dy));
                    Some(CapturedEvent::MouseMove { x, y })
                }
                Event::XinputButtonPress(ev) => {
                    button_from_code(ev.detail).map(|button| CapturedEvent::MouseButton {
                        button,
                        pressed: true,
                    })
                }
                Event::XinputButtonRelease(ev) => {
                    button_from_code(ev.detail).map(|button| CapturedEvent::MouseButton {
                        button,
                        pressed: false,
                    })
                }
                Event::XinputKeyPress(ev) => Some(CapturedEvent::Key {
                    keycode: x11_keycode_to_evdev(ev.detail),
                    modifiers: KeyModifiers::NONE,
                    pressed: true,
                }),
                Event::XinputKeyRelease(ev) => Some(CapturedEvent::Key {
                    keycode: x11_keycode_to_evdev(ev.detail),
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
        // orqueste la captura (crate `core`) debe tolerar ese último evento
        // de cola antes de que el hilo termine.
    }
}

/// El keycode que reporta X11 es el keycode `evdev` del kernel más el
/// offset fijo de 8 que usa XKB (los primeros 8 códigos están reservados
/// desde X11 clásico). El wire format del protocolo viaja en `evdev` puro
/// — mismo espacio que reporta Wayland/`libei` nativamente — así que hay
/// que restar el offset acá; `x11::inject` lo vuelve a sumar antes de
/// mandarlo a `xtest_fake_input`.
const fn x11_keycode_to_evdev(detail: u32) -> u32 {
    detail.saturating_sub(8)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_position_accumulates_and_resets() {
        let position = SharedPosition::new(10, 20);
        assert_eq!(position.add(5, -3), (15, 17));
        assert_eq!(position.add(1, 1), (16, 18));
        position.reset(0, 0);
        assert_eq!(position.get(), (0, 0));
    }
}
