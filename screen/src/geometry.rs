/// Rectángulo de un monitor en coordenadas del escritorio virtual de un
/// equipo. Puede tener `x`/`y` negativos: en Windows los monitores a la
/// izquierda o arriba del primario tienen coordenadas negativas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonitorGeometry {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl MonitorGeometry {
    #[must_use]
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    #[must_use]
    pub const fn left(&self) -> i32 {
        self.x
    }

    /// Ningún monitor real se acerca a los ~2.1 mil millones de píxeles de
    /// ancho que harían falta para que esta conversión desborde.
    #[must_use]
    #[allow(clippy::cast_possible_wrap)]
    pub fn right(&self) -> i32 {
        self.x + self.width as i32
    }

    #[must_use]
    pub const fn top(&self) -> i32 {
        self.y
    }

    #[must_use]
    #[allow(clippy::cast_possible_wrap)]
    pub fn bottom(&self) -> i32 {
        self.y + self.height as i32
    }

    #[must_use]
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.left() && x < self.right() && y >= self.top() && y < self.bottom()
    }

    /// Ajusta `(x, y)` para que caiga dentro de este rectángulo — pega el
    /// punto a la pared más cercana en vez de dejarlo salir. Para cuando no
    /// hay ningún borde vecino enlazado en esa dirección: sin este freno,
    /// una posición acumulada a partir de deltas (mientras el control está
    /// en remoto) puede alejarse sin límite del escritorio real y mandar
    /// coordenadas sin sentido a inyectar del otro lado.
    #[must_use]
    pub fn clamp_point(&self, x: i32, y: i32) -> (i32, i32) {
        let x = x.clamp(self.left(), self.right() - 1);
        let y = y.clamp(self.top(), self.bottom() - 1);
        (x, y)
    }

    /// Rectángulo más pequeño que contiene a `self` y `other`.
    ///
    /// `right >= left` y `bottom >= top` siempre se cumplen aquí (son un
    /// `max` sobre valores que ya incluyen a `left`/`top` en el `min`), así
    /// que la resta nunca es negativa.
    #[must_use]
    #[allow(clippy::cast_sign_loss)]
    pub fn union(&self, other: &Self) -> Self {
        let left = self.left().min(other.left());
        let top = self.top().min(other.top());
        let right = self.right().max(other.right());
        let bottom = self.bottom().max(other.bottom());
        Self {
            x: left,
            y: top,
            width: (right - left) as u32,
            height: (bottom - top) as u32,
        }
    }
}

/// Conjunto de monitores de un mismo equipo, tal como los ve el sistema
/// operativo (posiciones absolutas dentro del escritorio virtual local).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VirtualDesktop {
    monitors: Vec<MonitorGeometry>,
}

impl VirtualDesktop {
    #[must_use]
    pub const fn new(monitors: Vec<MonitorGeometry>) -> Self {
        Self { monitors }
    }

    #[must_use]
    pub fn monitors(&self) -> &[MonitorGeometry] {
        &self.monitors
    }

    /// Rectángulo envolvente de todos los monitores; `None` si no hay
    /// ninguno configurado todavía.
    #[must_use]
    pub fn bounds(&self) -> Option<MonitorGeometry> {
        let mut iter = self.monitors.iter();
        let first = *iter.next()?;
        Some(iter.fold(first, |acc, m| acc.union(m)))
    }

    #[must_use]
    pub fn contains_point(&self, x: i32, y: i32) -> bool {
        self.monitors.iter().any(|m| m.contains(x, y))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_point_pins_out_of_bounds_coordinates_to_the_nearest_edge() {
        let monitor = MonitorGeometry::new(0, 0, 1920, 1080);
        assert_eq!(monitor.clamp_point(960, 540), (960, 540));
        assert_eq!(monitor.clamp_point(-500, 540), (0, 540));
        assert_eq!(monitor.clamp_point(5000, 540), (1919, 540));
        assert_eq!(monitor.clamp_point(960, -500), (960, 0));
        assert_eq!(monitor.clamp_point(960, 5000), (960, 1079));
    }

    #[test]
    fn single_monitor_contains_its_own_area_only() {
        let monitor = MonitorGeometry::new(0, 0, 1920, 1080);
        assert!(monitor.contains(0, 0));
        assert!(monitor.contains(1919, 1079));
        assert!(!monitor.contains(1920, 0));
        assert!(!monitor.contains(0, 1080));
        assert!(!monitor.contains(-1, 0));
    }

    #[test]
    fn bounds_union_two_side_by_side_monitors() {
        let desktop = VirtualDesktop::new(vec![
            MonitorGeometry::new(0, 0, 1920, 1080),
            MonitorGeometry::new(1920, 0, 1280, 1024),
        ]);
        let bounds = desktop.bounds().expect("debería haber límites");
        assert_eq!(bounds, MonitorGeometry::new(0, 0, 3200, 1080));
    }

    #[test]
    fn bounds_handle_monitor_with_negative_origin() {
        let desktop = VirtualDesktop::new(vec![
            MonitorGeometry::new(-1920, 0, 1920, 1080),
            MonitorGeometry::new(0, 0, 1920, 1080),
        ]);
        let bounds = desktop.bounds().expect("debería haber límites");
        assert_eq!(bounds, MonitorGeometry::new(-1920, 0, 3840, 1080));
    }

    #[test]
    fn empty_desktop_has_no_bounds() {
        assert_eq!(VirtualDesktop::default().bounds(), None);
    }
}
