//! Backend de Wayland vía el portal `RemoteDesktop` de `xdg-desktop-portal`
//! (GNOME ≥ 45 / KDE Plasma ≥ 6.1).
//!
//! **Solo inyección.** Como documenta la Fase 0 de arquitectura (ver
//! `docs`/plan de diseño): capturar entrada en Wayland (ser el lado que
//! *emite* el control) es sustancialmente más frágil que recibirlo, y
//! requiere el portal `ExtInputCapture` (mucho más nuevo y con soporte de
//! compositor limitado) o protocolos específicos de wlroots
//! (`ext-input-capture-v1`) que no están implementados en esta fase.
//!
//! **Sin verificar en este entorno**: esta sesión de desarrollo corrió en
//! X11 (`XDG_SESSION_TYPE=x11`), sin compositor Wayland ni backend de
//! portal disponible para probar contra una sesión real. La API de `ashpd`
//! está implementada según su documentación, pero no se pudo ejercitar de
//! punta a punta.
mod inject;

pub use inject::WaylandPortalInjector;
