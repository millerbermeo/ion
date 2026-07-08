//! Captura e inyección de entrada (mouse+teclado) de `IonConnect`.
//!
//! Define los puertos [`InputCapture`]/[`InputInjector`] (hexagonal) y sus
//! adaptadores por sistema operativo. Selección de backend en runtime según
//! el sistema/sesión es responsabilidad de quien use este crate (`core`,
//! fase posterior) — aquí solo se exponen las implementaciones.

mod capture;
mod error;
mod event;
mod inject;

#[cfg(all(unix, not(target_os = "macos")))]
pub mod x11;

#[cfg(all(unix, not(target_os = "macos")))]
pub mod wayland;

#[cfg(windows)]
pub mod win32;

pub use capture::InputCapture;
pub use error::InputError;
pub use event::CapturedEvent;
pub use inject::InputInjector;
