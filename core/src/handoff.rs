use ionconnect_screen::Layout;
use ionconnect_shared::DeviceId;

/// A quién pertenece el control del mouse/teclado en este instante.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Active {
    /// El equipo local — el usuario controla su propia pantalla
    /// normalmente, sin interceptar nada.
    Local,
    /// Un equipo remoto — el puntero local está agarrado y todo se
    /// reenvía a este `DeviceId`.
    Remote(DeviceId),
}

/// Qué hacer en respuesta a un reporte de posición.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandoffAction {
    /// Empezar (o seguir) reenviando al equipo remoto, reapareciendo en
    /// `(x, y)` de su escritorio virtual.
    ForwardTo { device: DeviceId, x: i32, y: i32 },
    /// Devolver el control a este equipo, reapareciendo en `(x, y)` de su
    /// propio escritorio virtual.
    ReturnLocal { x: i32, y: i32 },
}

/// Máquina de estados que decide, a partir de la posición del cursor y la
/// distribución de pantallas ([`Layout`]), cuándo ceder el control del
/// mouse/teclado a otro equipo y a cuál.
///
/// Puramente lógica — no sabe nada de X11, red ni sockets. `core` la
/// alimenta con posiciones (absolutas mientras `Active::Local`, acumuladas
/// a partir de deltas mientras `Active::Remote`) y actúa según lo que
/// devuelva.
pub struct HandoffState {
    layout: Layout,
    local_device: DeviceId,
    active: Active,
}

impl HandoffState {
    #[must_use]
    pub const fn new(layout: Layout, local_device: DeviceId) -> Self {
        Self {
            layout,
            local_device,
            active: Active::Local,
        }
    }

    #[must_use]
    pub const fn active(&self) -> Active {
        self.active
    }

    /// Procesa un reporte de posición `(x, y)` — absoluta si `active() ==
    /// Local`, acumulada sobre el equipo activo si `active() ==
    /// Remote(_)`. Devuelve `Some(action)` si esto dispara un hand-off, y
    /// actualiza el estado interno en consecuencia.
    pub fn on_position(&mut self, x: i32, y: i32) -> Option<HandoffAction> {
        let current_device = match self.active {
            Active::Local => self.local_device,
            Active::Remote(device) => device,
        };
        let handoff = self.layout.detect_crossing(current_device, x, y)?;

        let action = if handoff.target_device == self.local_device {
            self.active = Active::Local;
            HandoffAction::ReturnLocal {
                x: handoff.x,
                y: handoff.y,
            }
        } else {
            self.active = Active::Remote(handoff.target_device);
            HandoffAction::ForwardTo {
                device: handoff.target_device,
                x: handoff.x,
                y: handoff.y,
            }
        };
        Some(action)
    }

    /// Fuerza la vuelta a `Local` si el control está actualmente cedido a
    /// `device` — para cuando ese peer se desconectó o dejó de responder y
    /// no hay forma de que dispare un cruce de borde normal. No hace nada
    /// (devuelve `false`) si el control ya es local o está en otro
    /// dispositivo, para no pisar un hand-off legítimo hacia otro peer.
    pub fn reclaim_if_remote(&mut self, device: DeviceId) -> bool {
        if self.active == Active::Remote(device) {
            self.active = Active::Local;
            true
        } else {
            false
        }
    }

    /// Ajusta `(x, y)` para que caiga dentro del escritorio virtual del
    /// dispositivo activo — usar en cada posición que se vaya a reenviar
    /// mientras `active() == Remote(_)`. Sin esto, un borde sin vecino
    /// enlazado (p. ej. arriba/abajo en un layout que solo conecta
    /// izquierda/derecha) deja que la posición acumulada a partir de
    /// deltas del mouse físico se aleje sin límite del escritorio real del
    /// peer — coordenadas como `y = 5000` en una pantalla de 1440px, que
    /// del otro lado se traducen en clics/movimientos sin sentido. Si no
    /// hay geometría conocida para el dispositivo activo, devuelve
    /// `(x, y)` sin tocar.
    #[must_use]
    pub fn clamp_to_active_desktop(&self, x: i32, y: i32) -> (i32, i32) {
        let device = match self.active {
            Active::Local => self.local_device,
            Active::Remote(device) => device,
        };
        self.layout
            .desktop(device)
            .and_then(ionconnect_screen::VirtualDesktop::bounds)
            .map_or((x, y), |bounds| bounds.clamp_point(x, y))
    }
}

#[cfg(test)]
mod tests {
    use ionconnect_screen::{MonitorGeometry, ScreenEdge, VirtualDesktop};

    use super::*;

    fn desktop(width: u32, height: u32) -> VirtualDesktop {
        VirtualDesktop::new(vec![MonitorGeometry::new(0, 0, width, height)])
    }

    fn layout_with_one_neighbor(local: DeviceId, remote: DeviceId) -> Layout {
        let mut layout = Layout::new();
        layout.set_desktop(local, desktop(1920, 1080));
        layout.set_desktop(remote, desktop(1920, 1080));
        layout.link_mirrored(local, ScreenEdge::Right, remote);
        layout
    }

    #[test]
    fn stays_local_while_inside_bounds() {
        let local = DeviceId::new();
        let remote = DeviceId::new();
        let mut state = HandoffState::new(layout_with_one_neighbor(local, remote), local);
        assert_eq!(state.on_position(960, 540), None);
        assert_eq!(state.active(), Active::Local);
    }

    #[test]
    fn crossing_right_edge_forwards_to_remote() {
        let local = DeviceId::new();
        let remote = DeviceId::new();
        let mut state = HandoffState::new(layout_with_one_neighbor(local, remote), local);

        let action = state
            .on_position(1920, 540)
            .expect("debería ceder el control");
        assert_eq!(
            action,
            HandoffAction::ForwardTo {
                device: remote,
                x: 0,
                y: 540
            }
        );
        assert_eq!(state.active(), Active::Remote(remote));
    }

    #[test]
    fn crossing_back_returns_control_locally() {
        let local = DeviceId::new();
        let remote = DeviceId::new();
        let mut state = HandoffState::new(layout_with_one_neighbor(local, remote), local);

        state.on_position(1920, 540);
        assert_eq!(state.active(), Active::Remote(remote));

        // Posición acumulada sobre la pantalla del remoto: cruza de vuelta
        // por su borde izquierdo.
        let action = state
            .on_position(-1, 300)
            .expect("debería devolver el control");
        assert_eq!(action, HandoffAction::ReturnLocal { x: 1919, y: 300 });
        assert_eq!(state.active(), Active::Local);
    }

    #[test]
    fn clamp_to_active_desktop_pins_runaway_coordinates_on_the_linked_axis_only() {
        let local = DeviceId::new();
        let remote = DeviceId::new();
        // Solo se enlaza el borde derecho/izquierdo (típico "uno al lado
        // del otro") — arriba/abajo se queda sin vecino, que es
        // justamente el caso que se escapaba sin el clamp.
        let mut state = HandoffState::new(layout_with_one_neighbor(local, remote), local);
        state.on_position(1920, 540);
        assert_eq!(state.active(), Active::Remote(remote));

        // Un delta enorme hacia abajo, muy por fuera de los 1080px de la
        // pantalla del remoto asumida — sin vecino en ese borde, antes se
        // dejaba crecer sin límite.
        assert_eq!(state.clamp_to_active_desktop(500, 5000), (500, 1079));
        assert_eq!(state.clamp_to_active_desktop(500, -5000), (500, 0));
        // Dentro de límites: no toca nada.
        assert_eq!(state.clamp_to_active_desktop(500, 300), (500, 300));
    }

    #[test]
    fn reclaim_if_remote_returns_to_local_only_for_the_active_device() {
        let local = DeviceId::new();
        let remote = DeviceId::new();
        let other = DeviceId::new();
        let mut state = HandoffState::new(layout_with_one_neighbor(local, remote), local);
        state.on_position(1920, 540);
        assert_eq!(state.active(), Active::Remote(remote));

        // Un dispositivo que no es el activo no debería poder reclamar.
        assert!(!state.reclaim_if_remote(other));
        assert_eq!(state.active(), Active::Remote(remote));

        assert!(state.reclaim_if_remote(remote));
        assert_eq!(state.active(), Active::Local);

        // Ya está local — no hay nada que reclamar.
        assert!(!state.reclaim_if_remote(remote));
    }

    #[test]
    fn no_handoff_when_target_edge_has_no_neighbor() {
        let local = DeviceId::new();
        let remote = DeviceId::new();
        let mut state = HandoffState::new(layout_with_one_neighbor(local, remote), local);

        // El borde izquierdo de `local` no tiene vecino configurado.
        assert_eq!(state.on_position(-1, 540), None);
        assert_eq!(state.active(), Active::Local);
    }

    #[test]
    fn chains_across_multiple_neighbors() {
        let local = DeviceId::new();
        let middle = DeviceId::new();
        let far = DeviceId::new();

        let mut layout = Layout::new();
        layout.set_desktop(local, desktop(1920, 1080));
        layout.set_desktop(middle, desktop(1920, 1080));
        layout.set_desktop(far, desktop(1920, 1080));
        layout.link_mirrored(local, ScreenEdge::Right, middle);
        layout.link_mirrored(middle, ScreenEdge::Right, far);

        let mut state = HandoffState::new(layout, local);

        let first = state.on_position(1920, 100).expect("primer hand-off");
        assert_eq!(
            first,
            HandoffAction::ForwardTo {
                device: middle,
                x: 0,
                y: 100
            }
        );

        // Sigue moviéndose a la derecha dentro de la pantalla de `middle`
        // hasta cruzar también su borde derecho.
        let second = state.on_position(1920, 100).expect("segundo hand-off");
        assert_eq!(
            second,
            HandoffAction::ForwardTo {
                device: far,
                x: 0,
                y: 100
            }
        );
        assert_eq!(state.active(), Active::Remote(far));
    }
}
