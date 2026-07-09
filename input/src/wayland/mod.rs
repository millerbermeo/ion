//! Backend de Wayland vía los portales `RemoteDesktop` (inyección) e
//! `InputCapture` (captura) de `xdg-desktop-portal` (GNOME ≥ 46 / KDE
//! Plasma ≥ 6.1 para captura; `RemoteDesktop` tiene soporte más viejo).
//!
//! Captura usa `InputCapture` + `libei` (protocolo `ext-input-capture-v1`
//! por debajo) — el compositor decide cuándo se cruzó una barrera de
//! borde y desde ahí transmite eventos de entrada por un socket EIS,
//! decodificado acá con el crate `reis`.
mod capture;
mod inject;

pub use capture::{BarrierSpec, CaptureZone, WaylandCaptureEvent, WaylandCaptureSession};
pub use inject::WaylandPortalInjector;
