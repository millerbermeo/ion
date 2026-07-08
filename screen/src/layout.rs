use std::collections::HashMap;

use ionconnect_shared::DeviceId;

use crate::edge::ScreenEdge;
use crate::geometry::{MonitorGeometry, VirtualDesktop};

/// A qué equipo y por qué borde debe reaparecer el cursor tras cruzar un
/// borde de pantalla.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LinkTarget {
    device: DeviceId,
    edge: ScreenEdge,
}

/// Resultado de un cruce de borde: a qué equipo pasa el control y en qué
/// posición absoluta debe reaparecer el cursor en su escritorio virtual.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Handoff {
    pub target_device: DeviceId,
    pub x: i32,
    pub y: i32,
}

/// Distribución espacial de los escritorios virtuales de todos los equipos
/// de la red, y qué borde de cuál equipo conecta con cuál otro. La arma la
/// GUI (el usuario arrastra representaciones de pantallas), la consulta
/// `core` en cada `MouseMove` para decidir si hay que ceder el control.
#[derive(Debug, Clone, Default)]
pub struct Layout {
    desktops: HashMap<DeviceId, VirtualDesktop>,
    links: HashMap<(DeviceId, ScreenEdge), LinkTarget>,
}

impl Layout {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_desktop(&mut self, device: DeviceId, desktop: VirtualDesktop) {
        self.desktops.insert(device, desktop);
    }

    #[must_use]
    pub fn desktop(&self, device: DeviceId) -> Option<&VirtualDesktop> {
        self.desktops.get(&device)
    }

    /// Enlaza un borde de `from` con un borde de `to`, en una sola
    /// dirección. Para el caso típico (pantallas lado a lado, el control
    /// vuelve por donde entró) usar [`Layout::link_mirrored`].
    pub fn link(
        &mut self,
        from: DeviceId,
        from_edge: ScreenEdge,
        to: DeviceId,
        to_edge: ScreenEdge,
    ) {
        self.links.insert(
            (from, from_edge),
            LinkTarget {
                device: to,
                edge: to_edge,
            },
        );
    }

    /// Enlaza `a`/`from_edge_on_a` con `b` en ambas direcciones, asumiendo
    /// el arreglo espacial más común: el borde de reingreso en el otro
    /// equipo es el opuesto al que se cruzó.
    pub fn link_mirrored(&mut self, a: DeviceId, from_edge_on_a: ScreenEdge, b: DeviceId) {
        let opposite = from_edge_on_a.opposite();
        self.link(a, from_edge_on_a, b, opposite);
        self.link(b, opposite, a, from_edge_on_a);
    }

    /// Si `(x, y)` sobre el escritorio de `device` cayó fuera de sus
    /// límites, determina a qué equipo transferir el control y en qué
    /// posición debe reaparecer el cursor allí. `None` si `(x, y)` sigue
    /// dentro de los límites de `device`, o si ese borde no tiene un equipo
    /// vecino configurado.
    #[must_use]
    pub fn detect_crossing(&self, device: DeviceId, x: i32, y: i32) -> Option<Handoff> {
        let bounds = self.desktops.get(&device)?.bounds()?;
        let edge = crossed_edge(&bounds, x, y)?;
        let target = self.links.get(&(device, edge))?;
        let target_bounds = self.desktops.get(&target.device)?.bounds()?;

        let along = along_edge_fraction(edge, &bounds, x, y);
        let (entry_x, entry_y) = entry_point(target.edge, &target_bounds, along);

        Some(Handoff {
            target_device: target.device,
            x: entry_x,
            y: entry_y,
        })
    }
}

/// `None` si `(x, y)` está dentro de `bounds`; de lo contrario, cuál borde
/// cruzó. Cuando `(x, y)` está fuera de los límites en dos ejes a la vez
/// (una esquina), prioriza el eje horizontal — es una elección arbitraria
/// pero consistente, un caso límite raro en la práctica.
fn crossed_edge(bounds: &MonitorGeometry, x: i32, y: i32) -> Option<ScreenEdge> {
    if x < bounds.left() {
        Some(ScreenEdge::Left)
    } else if x >= bounds.right() {
        Some(ScreenEdge::Right)
    } else if y < bounds.top() {
        Some(ScreenEdge::Top)
    } else if y >= bounds.bottom() {
        Some(ScreenEdge::Bottom)
    } else {
        None
    }
}

/// Posición relativa (0.0..=1.0) a lo largo del borde cruzado, para
/// preservar en el equipo destino en qué punto del borde iba el cursor
/// (p. ej. cruzar por la mitad de la pantalla entra por la mitad del lado
/// vecino, no siempre por la esquina).
fn along_edge_fraction(edge: ScreenEdge, bounds: &MonitorGeometry, x: i32, y: i32) -> f64 {
    if edge.is_vertical() {
        let span = f64::from(bounds.height.max(1));
        f64::from(y - bounds.top()) / span
    } else {
        let span = f64::from(bounds.width.max(1));
        f64::from(x - bounds.left()) / span
    }
    .clamp(0.0, 1.0)
}

/// Coordenada absoluta de reingreso en el equipo destino, dado el borde por
/// el que reaparece y la fracción a lo largo de ese borde.
///
/// `along` está acotado a `0.0..=1.0` por `along_edge_fraction`, así que el
/// resultado siempre cae dentro del ancho/alto del monitor — no hay
/// pérdida de precisión relevante para coordenadas de pantalla.
#[allow(clippy::cast_possible_truncation)]
fn entry_point(edge: ScreenEdge, bounds: &MonitorGeometry, along: f64) -> (i32, i32) {
    if edge.is_vertical() {
        let y = bounds.top() + (along * f64::from(bounds.height)) as i32;
        let y = y.clamp(bounds.top(), bounds.bottom() - 1);
        let x = if edge == ScreenEdge::Left {
            bounds.left()
        } else {
            bounds.right() - 1
        };
        (x, y)
    } else {
        let x = bounds.left() + (along * f64::from(bounds.width)) as i32;
        let x = x.clamp(bounds.left(), bounds.right() - 1);
        let y = if edge == ScreenEdge::Top {
            bounds.top()
        } else {
            bounds.bottom() - 1
        };
        (x, y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desktop(x: i32, y: i32, width: u32, height: u32) -> VirtualDesktop {
        VirtualDesktop::new(vec![MonitorGeometry::new(x, y, width, height)])
    }

    #[test]
    fn no_handoff_while_inside_bounds() {
        let mut layout = Layout::new();
        let a = DeviceId::new();
        layout.set_desktop(a, desktop(0, 0, 1920, 1080));
        assert_eq!(layout.detect_crossing(a, 960, 540), None);
    }

    #[test]
    fn no_handoff_at_unlinked_edge() {
        let mut layout = Layout::new();
        let a = DeviceId::new();
        layout.set_desktop(a, desktop(0, 0, 1920, 1080));
        // Cruza el borde derecho, pero no hay ningún link configurado.
        assert_eq!(layout.detect_crossing(a, 1920, 540), None);
    }

    #[test]
    fn handoff_to_mirrored_neighbor_preserves_relative_position() {
        let mut layout = Layout::new();
        let a = DeviceId::new();
        let b = DeviceId::new();
        layout.set_desktop(a, desktop(0, 0, 1920, 1080));
        layout.set_desktop(b, desktop(0, 0, 1280, 1024));
        layout.link_mirrored(a, ScreenEdge::Right, b);

        // Cruza por la mitad vertical del borde derecho de A (y=540 de 1080 → 50%).
        let handoff = layout
            .detect_crossing(a, 1920, 540)
            .expect("debería haber handoff");
        assert_eq!(handoff.target_device, b);
        // Reingresa por el borde izquierdo de B, a la misma fracción (50% de 1024 = 512).
        assert_eq!(handoff.x, 0);
        assert_eq!(handoff.y, 512);
    }

    #[test]
    fn handoff_back_from_mirrored_neighbor() {
        let mut layout = Layout::new();
        let a = DeviceId::new();
        let b = DeviceId::new();
        layout.set_desktop(a, desktop(0, 0, 1920, 1080));
        layout.set_desktop(b, desktop(0, 0, 1280, 1024));
        layout.link_mirrored(a, ScreenEdge::Right, b);

        // B está a la derecha de A; salir por la izquierda de B vuelve a A.
        let handoff = layout
            .detect_crossing(b, -1, 256)
            .expect("debería haber handoff de vuelta");
        assert_eq!(handoff.target_device, a);
        assert_eq!(handoff.x, 1919);
    }

    #[test]
    fn handoff_across_top_bottom_edges() {
        let mut layout = Layout::new();
        let a = DeviceId::new();
        let b = DeviceId::new();
        layout.set_desktop(a, desktop(0, 0, 1920, 1080));
        layout.set_desktop(b, desktop(0, 0, 1920, 1080));
        layout.link_mirrored(a, ScreenEdge::Top, b);

        let handoff = layout
            .detect_crossing(a, 480, -1)
            .expect("debería haber handoff");
        assert_eq!(handoff.target_device, b);
        assert_eq!(handoff.y, 1079);
        assert_eq!(handoff.x, 480);
    }

    #[test]
    fn handoff_to_differently_sized_target_edge() {
        // El vínculo no tiene por qué ser al borde opuesto: un layout en L
        // puede enlazar el borde derecho de A con el borde superior de C.
        let mut layout = Layout::new();
        let a = DeviceId::new();
        let c = DeviceId::new();
        layout.set_desktop(a, desktop(0, 0, 1000, 1000));
        layout.set_desktop(c, desktop(0, 0, 2000, 500));
        layout.link(a, ScreenEdge::Right, c, ScreenEdge::Top);

        // Cruza a 3/4 de la altura del borde derecho de A.
        let handoff = layout
            .detect_crossing(a, 1000, 750)
            .expect("debería haber handoff");
        assert_eq!(handoff.target_device, c);
        assert_eq!(handoff.y, 0);
        assert_eq!(handoff.x, 1500);
    }
}
