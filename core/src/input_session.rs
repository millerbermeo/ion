use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ionconnect_input::wayland::{BarrierSpec, WaylandCaptureEvent, WaylandCaptureSession};
use ionconnect_input::x11::{SharedPosition, X11Capture, X11Control};
use ionconnect_input::{CapturedEvent, InputCapture as _, InputError};
use ionconnect_protocol::{
    KeyboardPress, KeyboardRelease, Message, MouseButton, MouseClick, MouseMove,
};
use ionconnect_shared::DeviceId;
use tracing::{info, warn};

use crate::handoff::{Active, HandoffAction, HandoffState};
use crate::routing::Routing;
use crate::udp_peers::UdpPeers;

/// Filtra pulsaciones/clics duplicados que a veces reporta la captura del
/// sistema operativo para una sola acción física real (ver
/// `ionconnect_input::x11::X11Capture` para el porqué puede pasar en X11).
/// Se resuelve acá, del lado de `core`, en vez de ajustar qué eventos
/// selecciona cada backend de captura — más robusto porque no depende de
/// la semántica fina de cada plataforma, y funciona igual sin importar de
/// dónde venga la duplicación.
#[derive(Default)]
struct HeldGuard {
    keys: std::collections::HashSet<u32>,
    buttons: std::collections::HashSet<MouseButton>,
}

impl HeldGuard {
    /// `true` si `event` es una transición de estado nueva y hay que
    /// procesarla; `false` si es un duplicado exacto del último reporte
    /// para esa tecla/botón (un `pressed: true` repetido sin soltar antes,
    /// o un `pressed: false` repetido sin haber estado presionado) y hay
    /// que descartarlo en silencio. Siempre `true` para eventos que no son
    /// de tecla/botón.
    fn accept(&mut self, event: &CapturedEvent) -> bool {
        match *event {
            CapturedEvent::Key {
                keycode,
                pressed: true,
                ..
            } => self.keys.insert(keycode),
            CapturedEvent::Key {
                keycode,
                pressed: false,
                ..
            } => self.keys.remove(&keycode),
            CapturedEvent::MouseButton {
                button,
                pressed: true,
            } => self.buttons.insert(button),
            CapturedEvent::MouseButton {
                button,
                pressed: false,
            } => self.buttons.remove(&button),
            CapturedEvent::AbsolutePosition { .. } | CapturedEvent::MouseMove { .. } => true,
        }
    }
}

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
    udp_peers: &Arc<UdpPeers>,
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

    let mut held = HeldGuard::default();
    while let Ok(event) = rx.recv() {
        handle_captured_event(
            event, handoff, &control, &position, routing, udp_peers, &mut held,
        );
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

/// Si el control está cedido a un peer que ya no tiene conexión registrada
/// en `routing` (se desconectó, crasheó, o se le mató el proceso), lo
/// recupera localmente — sin esto, un peer que desaparece mientras tiene
/// el control deja el mouse/teclado local muertos para siempre, porque
/// nada vuelve a disparar un cruce de borde. Se llama en cada evento
/// capturado, así que la detección es prácticamente instantánea mientras
/// haya algo de actividad (mover el mouse alcanza).
fn reclaim_if_peer_gone(
    handoff: &Arc<Mutex<HandoffState>>,
    control: &X11Control,
    position: &SharedPosition,
    routing: &Routing,
) {
    let device = match handoff
        .lock()
        .expect("el lock de handoff no debería estar envenenado")
        .active()
    {
        Active::Remote(device) => device,
        Active::Local => return,
    };
    if routing.is_connected(device) {
        return;
    }
    warn!(
        %device,
        "peer remoto desconectado con el control activo — recuperando control local"
    );
    let reclaimed = handoff
        .lock()
        .expect("el lock de handoff no debería estar envenenado")
        .reclaim_if_remote(device);
    if reclaimed {
        let (x, y) = position.get();
        apply_handoff_action(
            HandoffAction::ReturnLocal { x, y },
            handoff,
            control,
            position,
            routing,
            (x, y),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_captured_event(
    event: CapturedEvent,
    handoff: &Arc<Mutex<HandoffState>>,
    control: &X11Control,
    position: &SharedPosition,
    routing: &Routing,
    udp_peers: &UdpPeers,
    held: &mut HeldGuard,
) {
    reclaim_if_peer_gone(handoff, control, position, routing);
    match event {
        CapturedEvent::AbsolutePosition { x, y } | CapturedEvent::MouseMove { x, y } => {
            if POSITION_LOG_COUNTER.fetch_add(1, Ordering::Relaxed) % POSITION_LOG_SAMPLE_RATE == 0
            {
                info!(x, y, ?event, "posición de mouse capturada");
            }
            handle_position_report(event, x, y, handoff, control, position, routing, udp_peers);
        }
        CapturedEvent::MouseButton { .. } | CapturedEvent::Key { .. } => {
            if !held.accept(&event) {
                return;
            }
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
#[allow(clippy::too_many_arguments)]
fn handle_position_report(
    event: CapturedEvent,
    x: i32,
    y: i32,
    handoff: &Arc<Mutex<HandoffState>>,
    control: &X11Control,
    position: &SharedPosition,
    routing: &Routing,
    udp_peers: &UdpPeers,
) {
    let mut state = handoff
        .lock()
        .expect("el lock de handoff no debería estar envenenado");

    // Mientras el control es local, X11 clava el cursor real en el último
    // píxel válido de la pantalla (nunca reporta `x >= width` ni `x < 0`),
    // así que `AbsolutePosition` sola nunca dispara un cruce de borde acá
    // — el usuario queda con el mouse pegado al borde para siempre.
    // Resincronizar `position` en cada reporte absoluto deja que, apenas
    // el cursor visual queda pinneado contra el borde, los deltas crudos
    // de `MouseMove` que siguen llegando (sin ese clamp, ver
    // `X11Capture`) se acumulen *desde ahí* y sí superen el límite.
    if state.active() == Active::Local
        && let CapturedEvent::AbsolutePosition { .. } = event
    {
        position.reset(x, y);
    }

    let relevant = matches!(
        (state.active(), event),
        (
            Active::Local,
            CapturedEvent::AbsolutePosition { .. } | CapturedEvent::MouseMove { .. }
        ) | (Active::Remote(_), CapturedEvent::MouseMove { .. })
    );
    if !relevant {
        return;
    }

    if let Some(action) = state.on_position(x, y) {
        drop(state);
        apply_handoff_action(action, handoff, control, position, routing, (x, y));
    } else if let Active::Remote(device) = state.active() {
        // Sin vecino enlazado en el borde que se acaba de cruzar (o
        // todavía dentro de límites): igual hay que pegar la posición al
        // escritorio del remoto, si no la acumulada sigue alejándose sin
        // límite — ver la documentación de `clamp_to_active_desktop`.
        let (x, y) = state.clamp_to_active_desktop(x, y);
        drop(state);
        position.reset(x, y);
        // Delta continuo (no el primer `MouseMove` de un hand-off, que va
        // por `apply_handoff_action` y siempre por TCP): tolera perderse,
        // el próximo lo reemplaza — ver `core::udp_peers`. Si no hay peer
        // UDP registrado (todavía no llegó su `UdpHello`, o no lo soporta),
        // cae a la conexión confiable de siempre.
        if !udp_peers.try_send_mouse_move(device, x, y) {
            routing.send_to(device, Message::MouseMove(MouseMove { x, y }));
        }
    }
}

/// `local_xy` es la posición real en la pantalla *local* en el instante del
/// hand-off (no confundir con `x, y` dentro de `HandoffAction::ForwardTo`,
/// que ya están expresados en el escritorio del equipo remoto) — se usa
/// para clavar ahí el cursor real vía [`X11Control::grab`] y que no se
/// pasee por toda la pantalla local mientras el control ya es del remoto.
fn apply_handoff_action(
    action: HandoffAction,
    handoff: &Arc<Mutex<HandoffState>>,
    control: &X11Control,
    position: &SharedPosition,
    routing: &Routing,
    local_xy: (i32, i32),
) {
    match action {
        HandoffAction::ForwardTo { device, x, y } => {
            info!(%device, x, y, "hand-off: cediendo control a equipo remoto");
            if let Err(err) = control.grab(local_xy.0, local_xy.1) {
                // `HandoffState::on_position` ya marcó el estado como
                // `Remote` antes de esto (necesita decidir el hand-off
                // antes de saber si el grab real va a funcionar). Si el
                // grab falla — típicamente porque otro cliente X11 (p. ej.
                // GNOME Shell con su vista de actividades abierta) ya tiene
                // el puntero agarrado — el cursor real queda sin confinar
                // pero el estado interno sigue pensando que el control es
                // remoto, y arranca a reenviar movimiento igual: se ve
                // como si el mouse se moviera en las dos pantallas a la
                // vez. Sin este `reclaim_if_remote`, ese estado fantasma
                // queda pegado hasta el próximo cruce de borde real.
                warn!(%err, "no se pudo agarrar el puntero para el hand-off, revirtiendo a control local");
                handoff
                    .lock()
                    .expect("el lock de handoff no debería estar envenenado")
                    .reclaim_if_remote(device);
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
    udp_peers: Arc<UdpPeers>,
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
    let mut held = HeldGuard::default();

    // Red de seguridad para cuando el peer que tiene el control desaparece
    // (crash, se le mata el proceso, se corta la red) sin que eso dispare
    // un evento de captura — p. ej. si nadie está tocando el mouse en ese
    // momento. Sin este tick, `next_event` se queda esperando para siempre
    // y el control local nunca vuelve. Cada evento capturado también
    // dispara el mismo chequeo (ver `handle_wayland_input`), así que en la
    // práctica la recuperación es casi instantánea apenas hay actividad;
    // esto solo cubre el caso de inactividad total.
    let mut watchdog = tokio::time::interval(Duration::from_secs(1));

    loop {
        tokio::select! {
            _ = watchdog.tick() => {
                reclaim_if_peer_gone_wayland(
                    &handoff,
                    &routing,
                    &mut session,
                    &mut current_activation,
                )
                .await;
            }
            event = session.next_event(&mut activated_stream, &mut deactivated_stream) => {
                let event = event?;
                handle_wayland_event(
                    event,
                    &mut session,
                    &handoff,
                    &routing,
                    &udp_peers,
                    &mut current_activation,
                    &mut held,
                )
                .await;
            }
        }
    }
}

async fn reclaim_if_peer_gone_wayland(
    handoff: &Arc<Mutex<HandoffState>>,
    routing: &Routing,
    session: &mut WaylandCaptureSession,
    current_activation: &mut Option<u32>,
) {
    let device = match handoff
        .lock()
        .expect("el lock de handoff no debería estar envenenado")
        .active()
    {
        Active::Remote(device) => device,
        Active::Local => return,
    };
    if routing.is_connected(device) {
        return;
    }
    warn!(
        %device,
        "peer remoto desconectado con el control activo (Wayland) — recuperando control local"
    );
    let reclaimed = handoff
        .lock()
        .expect("el lock de handoff no debería estar envenenado")
        .reclaim_if_remote(device);
    if reclaimed {
        let _ = session.release(*current_activation, None).await;
        *current_activation = None;
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_wayland_event(
    event: WaylandCaptureEvent,
    session: &mut WaylandCaptureSession,
    handoff: &Arc<Mutex<HandoffState>>,
    routing: &Routing,
    udp_peers: &UdpPeers,
    current_activation: &mut Option<u32>,
    held: &mut HeldGuard,
) {
    match event {
        WaylandCaptureEvent::Activated {
            activation_id,
            cursor,
            ..
        } => {
            *current_activation = activation_id;
            let Some((x, y)) = cursor else {
                warn!("activación sin posición de cursor reportada, ignorando");
                return;
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
                    let _ = session.release(*current_activation, None).await;
                    *current_activation = None;
                }
            }
        }
        WaylandCaptureEvent::Deactivated { .. } => {
            info!("captura Wayland desactivada");
            *current_activation = None;
        }
        WaylandCaptureEvent::Input(captured) => {
            handle_wayland_input(
                captured,
                session,
                handoff,
                routing,
                udp_peers,
                current_activation,
                held,
            )
            .await;
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_wayland_input(
    event: CapturedEvent,
    session: &mut WaylandCaptureSession,
    handoff: &Arc<Mutex<HandoffState>>,
    routing: &Routing,
    udp_peers: &UdpPeers,
    current_activation: &mut Option<u32>,
    held: &mut HeldGuard,
) {
    reclaim_if_peer_gone_wayland(handoff, routing, session, current_activation).await;

    if matches!(
        event,
        CapturedEvent::MouseButton { .. } | CapturedEvent::Key { .. }
    ) && !held.accept(&event)
    {
        return;
    }

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
            let mut guard = handoff
                .lock()
                .expect("el lock de handoff no debería estar envenenado");
            let action = guard.on_position(x, y);
            if let Some(HandoffAction::ReturnLocal { x, y }) = action {
                drop(guard);
                info!(x, y, "hand-off: recuperando control local");
                let _ = session
                    .release(*current_activation, Some((f64::from(x), f64::from(y))))
                    .await;
                *current_activation = None;
            } else {
                // Sin vecino enlazado en ese borde (o todavía dentro de
                // límites): pegar la posición al escritorio del remoto
                // para que la acumulada no se aleje sin límite — ver
                // `HandoffState::clamp_to_active_desktop`.
                let (x, y) = guard.clamp_to_active_desktop(x, y);
                drop(guard);
                session.reset_position(x, y);
                // Delta continuo — tolera perderse, ver el comentario
                // equivalente en `handle_position_report` (X11).
                if !udp_peers.try_send_mouse_move(device, x, y) {
                    routing.send_to(device, Message::MouseMove(MouseMove { x, y }));
                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use ionconnect_shared::KeyModifiers;

    fn key(keycode: u32, pressed: bool) -> CapturedEvent {
        CapturedEvent::Key {
            keycode,
            modifiers: KeyModifiers::NONE,
            pressed,
        }
    }

    #[test]
    fn drops_a_press_reported_twice_without_a_release_in_between() {
        let mut held = HeldGuard::default();
        assert!(held.accept(&key(30, true)), "primera pulsación: nueva");
        assert!(
            !held.accept(&key(30, true)),
            "segunda pulsación sin soltar antes: duplicado, se descarta"
        );
    }

    #[test]
    fn drops_a_release_reported_twice_without_a_press_in_between() {
        let mut held = HeldGuard::default();
        assert!(held.accept(&key(30, true)));
        assert!(held.accept(&key(30, false)), "primer release: nuevo");
        assert!(
            !held.accept(&key(30, false)),
            "segundo release sin volver a presionar: duplicado, se descarta"
        );
    }

    #[test]
    fn accepts_legitimate_repeated_taps_of_the_same_key() {
        let mut held = HeldGuard::default();
        // Tap, tap, tap — cada uno con su release intermedio, como escribir
        // la misma letra varias veces seguidas. No debería filtrarse nada.
        for _ in 0..3 {
            assert!(held.accept(&key(30, true)));
            assert!(held.accept(&key(30, false)));
        }
    }

    #[test]
    fn tracks_keys_and_buttons_independently() {
        let mut held = HeldGuard::default();
        assert!(held.accept(&key(30, true)));
        assert!(held.accept(&CapturedEvent::MouseButton {
            button: MouseButton::Left,
            pressed: true,
        }));
        // El botón repetido se descarta; la tecla, ya soltada aparte, sigue
        // pudiendo volver a presionarse — estados independientes.
        assert!(!held.accept(&CapturedEvent::MouseButton {
            button: MouseButton::Left,
            pressed: true,
        }));
        assert!(held.accept(&key(30, false)));
        assert!(held.accept(&key(30, true)));
    }

    #[test]
    fn always_accepts_motion_events() {
        let mut held = HeldGuard::default();
        assert!(held.accept(&CapturedEvent::MouseMove { x: 1, y: 1 }));
        assert!(held.accept(&CapturedEvent::MouseMove { x: 1, y: 1 }));
        assert!(held.accept(&CapturedEvent::AbsolutePosition { x: 1, y: 1 }));
    }
}
