//! Geometría de monitores, distribución espacial multi-equipo y detección
//! de cruce de borde para el hand-off del cursor entre equipos.
//!
//! Puramente lógico — no toca la API de ningún sistema operativo. Obtener
//! la geometría real de los monitores (`XRandR`, `EnumDisplayMonitors`,
//! etc.) es responsabilidad de quien use este crate.

mod edge;
mod geometry;
mod layout;

pub use edge::ScreenEdge;
pub use geometry::{MonitorGeometry, VirtualDesktop};
pub use layout::{Handoff, Layout};
