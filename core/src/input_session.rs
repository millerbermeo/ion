use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};

use ionconnect_input::wayland::{BarrierSpec, WaylandCaptureEvent, WaylandCaptureSession};
use ionconnect_input::x11::{SharedPosition, X11Capture, X11Control};
use ionconnect_input::{CapturedEvent, InputCapture as _, InputError};
use ionconnect_protocol::{KeyboardPress, KeyboardRelease, Message, MouseClick, MouseMove};
use ionconnect_shared::DeviceId;
use tracing::{info, warn};

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
    info!("captura de entrada X11 iniciada");

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

/// Cuántos reportes de posición saltear entre cada línea de log — a la
/// tasa normal de un mouse (cientos de eventos/s) loguear todos inundaría
/// el panel de la GUI; cada ~30 alcanza para confirmar en vivo que la
/// captura sigue viva sin ahogar el resto del log.
const POSITION_LOG_SAMPLE_RATE: u32 = 30;
static POSITION_LOG_COUNTER: AtomicU32 = AtomicU32::new(0);

fn handle_captured_event(
    event: CapturedEvent,
    handoff: &Arc<Mutex<HandoffState>>,
    control: &X11Control,
    position: &SharedPosition,
    routing: &Routing,
) {
    match event {
        CapturedEvent::AbsolutePosition { x, y } | CapturedEvent::MouseMove { x, y } => {
            if POSITION_LOG_COUNTER.fetch_add(1, Ordering::Relaxed) % POSITION_LOG_SAMPLE_RATE == 0
            {
                info!(x, y, ?event, "posición de mouse capturada");
            }
            handle_position_report(event, x, y, handoff, control, position, routing);
        }
        CapturedEvent::MouseButton { .. } | CapturedEvent::Key { .. } => {
            info!(?event, "botón/tecla capturado");
            let active = handoff
                .lock()
                .expect("el lock de handoff no debería estar envenenado")
                .active();
            if let Active::Remote(device) = active {
                forward_button_or_key(event, device, position, routing);
            } else {
                info!("botón/tecla no reenviado: control sigue local");
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
            info!(%device, x, y, "hand-off: cediendo control a equipo remoto");
            if let Err(err) = control.grab() {
                warn!(%err, "no se pudo agarrar el puntero para el hand-off");
                return;
            }
            position.reset(x, y);
            if !routing.send_to(device, Message::MouseMove(MouseMove { x, y })) {
                warn!(%device, "hand-off disparado pero el peer no está conectado en routing");
            }
        }
        HandoffAction::ReturnLocal { x, y } => {
            info!(x, y, "hand-off: recuperando control local");
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

/// Corre la sesión de captura Wayland (portal `InputCapture` + `libei`) —
/// async, a diferencia de la de X11: acá es el compositor el que decide
/// cuándo se cruzó un borde (vía las barreras que configuramos), no
/// nosotros sondeando posición contra nuestra propia geometría.
///
/// Reutiliza la misma [`HandoffState`] que la sesión X11: en la
/// activación, alimenta la posición real del cursor que reporta el portal
/// a `on_position` — misma lógica pura de `screen::Layout`, solo cambia
/// quién dispara la primera llamada.
///
/// # Errors
///
/// Devuelve [`InputError`] si registrar las barreras, habilitar la
/// captura, o el stream de eventos EIS fallan.
pub async fn run_wayland_input_session(
    mut session: WaylandCaptureSession,
    barriers: Vec<BarrierSpec>,
    handoff: Arc<Mutex<HandoffState>>,
    routing: Arc<Routing>,
) -> Result<(), InputError> {
    let failed = session.set_barriers(&barriers).await?;
    if !failed.is_empty() {
        warn!(
            ?failed,
            "algunas barreras de hand-off fueron rechazadas por el compositor"
        );
    }
    session.enable().await?;
    info!("captura de entrada Wayland habilitada");

    // Handle separado (misma conexión D-Bus, no una nueva) para no pelear
    // por el préstamo de `&mut session` que hace falta más abajo — ver
    // la documentación de `WaylandCaptureSession::activation_watcher`.
    let watcher = session.activation_watcher().await?;
    let mut activated_stream = watcher.receive_activated().await?;
    let mut deactivated_stream = watcher.receive_deactivated().await?;
    let mut current_activation: Option<u32> = None;

    loop {
        let event = session
            .next_event(&mut activated_stream, &mut deactivated_stream)
            .await?;
        match event {
            WaylandCaptureEvent::Activated {
                activation_id,
                cursor,
                ..
            } => {
                current_activation = activation_id;
                let Some((x, y)) = cursor else {
                    warn!("activación sin posición de cursor reportada, ignorando");
                    continue;
                };
                #[allow(clippy::cast_possible_truncation)]
                let (x, y) = (x as i32, y as i32);
                info!(x, y, "captura Wayland activada");

                let action = handoff
                    .lock()
                    .expect("el lock de handoff no debería estar envenenado")
                    .on_position(x, y);

                match action {
                    Some(HandoffAction::ForwardTo { device, x, y }) => {
                        info!(%device, x, y, "hand-off: cediendo control a equipo remoto");
                        session.reset_position(x, y);
                        if !routing.send_to(device, Message::MouseMove(MouseMove { x, y })) {
                            warn!(%device, "hand-off disparado pero el peer no está conectado en routing");
                        }
                    }
                    Some(HandoffAction::ReturnLocal { .. }) | None => {
                        warn!(
                            "activación sin hand-off válido (¿barrera sin vecino configurado en ese borde?), liberando"
                        );
                        let _ = session.release(current_activation, None).await;
                        current_activation = None;
                    }
                }
            }
            WaylandCaptureEvent::Deactivated { .. } => {
                info!("captura Wayland desactivada");
                current_activation = None;
            }
            WaylandCaptureEvent::Input(captured) => {
                handle_wayland_input(
                    captured,
                    &mut session,
                    &handoff,
                    &routing,
                    &mut current_activation,
                )
                .await;
            }
        }
    }
}

async fn handle_wayland_input(
    event: CapturedEvent,
    session: &mut WaylandCaptureSession,
    handoff: &Arc<Mutex<HandoffState>>,
    routing: &Routing,
    current_activation: &mut Option<u32>,
) {
    let active = handoff
        .lock()
        .expect("el lock de handoff no debería estar envenenado")
        .active();
    let Active::Remote(device) = active else {
        return;
    };

    match event {
        CapturedEvent::MouseMove { x, y } => {
            if POSITION_LOG_COUNTER.fetch_add(1, Ordering::Relaxed) % POSITION_LOG_SAMPLE_RATE == 0
            {
                info!(x, y, "posición de mouse capturada (Wayland)");
            }
            let action = handoff
                .lock()
                .expect("el lock de handoff no debería estar envenenado")
                .on_position(x, y);
            if let Some(HandoffAction::ReturnLocal { x, y }) = action {
                info!(x, y, "hand-off: recuperando control local");
                let _ = session
                    .release(*current_activation, Some((f64::from(x), f64::from(y))))
                    .await;
                *current_activation = None;
            } else {
                routing.send_to(device, Message::MouseMove(MouseMove { x, y }));
            }
        }
        CapturedEvent::MouseButton { button, pressed } => {
            info!(?button, pressed, "botón capturado (Wayland)");
            let (x, y) = session.position();
            routing.send_to(
                device,
                Message::MouseClick(MouseClick {
                    button,
                    pressed,
                    x,
                    y,
                }),
            );
        }
        CapturedEvent::Key {
            keycode,
            modifiers,
            pressed: true,
        } => {
            info!(keycode, "tecla capturada (Wayland)");
            routing.send_to(
                device,
                Message::KeyboardPress(KeyboardPress { keycode, modifiers }),
            );
        }
        CapturedEvent::Key {
            keycode,
            modifiers,
            pressed: false,
        } => {
            routing.send_to(
                device,
                Message::KeyboardRelease(KeyboardRelease { keycode, modifiers }),
            );
        }
        CapturedEvent::AbsolutePosition { .. } => {}
    }
}
