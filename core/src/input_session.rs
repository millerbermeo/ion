use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};

use ionconnect_input::x11::{SharedPosition, X11Capture, X11Control};
use ionconnect_input::{CapturedEvent, InputCapture as _, InputError};
use ionconnect_protocol::{KeyboardPress, KeyboardRelease, Message, MouseClick, MouseMove};
use ionconnect_shared::DeviceId;
use tracing::warn;

use crate::handoff::{Active, HandoffAction, HandoffState};
use crate::routing::Routing;

/// Corre la sesión de captura de entrada X11 en el hilo actual —
/// **bloqueante**: llamar desde `tokio::task::spawn_blocking`, nunca
/// directamente dentro de una tarea async (igual que
/// [`ionconnect_input::InputCapture::run`], del que depende).
///
/// Alimenta [`HandoffState`] con cada posición reportada; cuando eso
/// dispara un hand-off, agarra/suelta el puntero real y reenvía
/// mouse/teclado al peer activo vía [`Routing`].
///
/// # Errors
///
/// Devuelve [`InputError`] si no se pudo abrir alguna de las dos
/// conexiones X11 que hacen falta (una para capturar eventos, otra para
/// las órdenes de control — ver [`ionconnect_input::x11::X11Control`]).
pub fn run_x11_input_session(
    handoff: &Arc<Mutex<HandoffState>>,
    routing: &Arc<Routing>,
) -> Result<(), InputError> {
    let position = SharedPosition::new(0, 0);
    let mut capture = X11Capture::connect(position.clone())?;
    let control = X11Control::connect()?;

    let (tx, rx) = std_mpsc::channel();
    let capture_thread = std::thread::spawn(move || {
        if let Err(err) = capture.run(tx) {
            warn!(%err, "el hilo de captura X11 terminó con error");
        }
    });

    while let Ok(event) = rx.recv() {
        handle_captured_event(event, handoff, &control, &position, routing);
    }

    let _ = capture_thread.join();
    Ok(())
}

fn handle_captured_event(
    event: CapturedEvent,
    handoff: &Arc<Mutex<HandoffState>>,
    control: &X11Control,
    position: &SharedPosition,
    routing: &Routing,
) {
    match event {
        CapturedEvent::AbsolutePosition { x, y } | CapturedEvent::MouseMove { x, y } => {
            handle_position_report(event, x, y, handoff, control, position, routing);
        }
        CapturedEvent::MouseButton { .. } | CapturedEvent::Key { .. } => {
            let active = handoff
                .lock()
                .expect("el lock de handoff no debería estar envenenado")
                .active();
            if let Active::Remote(device) = active {
                forward_button_or_key(event, device, position, routing);
            }
        }
    }
}

/// Solo el tipo de reporte que corresponde al estado actual es relevante:
/// posición absoluta (no cruda) mientras `Local` — el sistema operativo
/// sigue moviendo el cursor real, y es lo único fiable ahí; posición
/// acumulada (cruda) mientras `Remote` — ver la documentación de
/// [`ionconnect_input::x11::X11Capture`] para el porqué.
fn handle_position_report(
    event: CapturedEvent,
    x: i32,
    y: i32,
    handoff: &Arc<Mutex<HandoffState>>,
    control: &X11Control,
    position: &SharedPosition,
    routing: &Routing,
) {
    let mut state = handoff
        .lock()
        .expect("el lock de handoff no debería estar envenenado");
    let relevant = matches!(
        (state.active(), event),
        (Active::Local, CapturedEvent::AbsolutePosition { .. })
            | (Active::Remote(_), CapturedEvent::MouseMove { .. })
    );
    if !relevant {
        return;
    }

    if let Some(action) = state.on_position(x, y) {
        drop(state);
        apply_handoff_action(action, control, position, routing);
    } else if let Active::Remote(device) = state.active() {
        drop(state);
        routing.send_to(device, Message::MouseMove(MouseMove { x, y }));
    }
}

fn apply_handoff_action(
    action: HandoffAction,
    control: &X11Control,
    position: &SharedPosition,
    routing: &Routing,
) {
    match action {
        HandoffAction::ForwardTo { device, x, y } => {
            if let Err(err) = control.grab() {
                warn!(%err, "no se pudo agarrar el puntero para el hand-off");
                return;
            }
            position.reset(x, y);
            routing.send_to(device, Message::MouseMove(MouseMove { x, y }));
        }
        HandoffAction::ReturnLocal { x, y } => {
            if let Err(err) = control.ungrab() {
                warn!(%err, "no se pudo soltar el puntero al devolver el control");
            }
            if let Err(err) = control.warp_to(x, y) {
                warn!(%err, "no se pudo mover el cursor real al devolver el control");
            }
        }
    }
}

fn forward_button_or_key(
    event: CapturedEvent,
    device: DeviceId,
    position: &SharedPosition,
    routing: &Routing,
) {
    let (x, y) = position.get();
    let message = match event {
        CapturedEvent::MouseButton { button, pressed } => Some(Message::MouseClick(MouseClick {
            button,
            pressed,
            x,
            y,
        })),
        CapturedEvent::Key {
            keycode,
            modifiers,
            pressed: true,
        } => Some(Message::KeyboardPress(KeyboardPress { keycode, modifiers })),
        CapturedEvent::Key {
            keycode,
            modifiers,
            pressed: false,
        } => Some(Message::KeyboardRelease(KeyboardRelease {
            keycode,
            modifiers,
        })),
        CapturedEvent::AbsolutePosition { .. } | CapturedEvent::MouseMove { .. } => None,
    };
    if let Some(message) = message {
        routing.send_to(device, message);
    }
}
