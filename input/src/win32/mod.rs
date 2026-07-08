//! Backend de Windows 11: `SetWindowsHookEx` (`WH_MOUSE_LL`/`WH_KEYBOARD_LL`)
//! para captura y `SendInput` para inyección.
//!
//! **Sin verificar en este entorno**: el resto de este proyecto se
//! desarrolló en Linux y no hay una máquina Windows disponible para
//! compilar ni ejercitar este módulo. La API de Win32 usada es estable
//! desde hace décadas y de bajo riesgo, pero antes de darlo por bueno hace
//! falta compilarlo y probarlo en Windows 11 real.

mod capture;
mod inject;

pub use capture::WindowsCapture;
pub use inject::WindowsInjector;
