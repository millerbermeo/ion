use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP, MOUSEEVENTF_ABSOLUTE,
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
    MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK,
    MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEINPUT, SendInput, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    XBUTTON1, XBUTTON2,
};

use ionconnect_protocol::MouseButton;

use crate::error::InputError;
use crate::event::CapturedEvent;
use crate::inject::InputInjector;

/// Inyector de Windows basado en `SendInput`. No mantiene estado propio: a
/// diferencia de la captura (que necesita un hilo con hook instalado),
/// inyectar es una llamada de una sola vez por evento.
#[derive(Debug, Default)]
pub struct WindowsInjector;

impl WindowsInjector {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

/// Convierte una coordenada absoluta de pantalla a la escala 0..=65535 que
/// `MOUSEEVENTF_ABSOLUTE` espera, relativa al escritorio virtual completo
/// (multi-monitor incluido).
fn normalize_absolute(value: i32, origin: i32, extent: i32) -> i32 {
    if extent <= 0 {
        return 0;
    }
    (((value - origin) as i64 * 65535) / i64::from(extent)) as i32
}

fn send_mouse(
    dx: i32,
    dy: i32,
    mouse_data: i32,
    flags: windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS,
) -> Result<(), InputError> {
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: mouse_data as u32,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let sent = unsafe { SendInput(&[input], size_of::<INPUT>() as i32) };
    if sent == 1 {
        Ok(())
    } else {
        Err(InputError::Windows(
            "SendInput (mouse) no encoló el evento".to_string(),
        ))
    }
}

fn send_key(vk: u16, key_up: bool) -> Result<(), InputError> {
    let mut flags = windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS(0);
    if key_up {
        flags |= KEYEVENTF_KEYUP;
    }
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let sent = unsafe { SendInput(&[input], size_of::<INPUT>() as i32) };
    if sent == 1 {
        Ok(())
    } else {
        Err(InputError::Windows(
            "SendInput (teclado) no encoló el evento".to_string(),
        ))
    }
}

impl InputInjector for WindowsInjector {
    fn inject(&mut self, event: &CapturedEvent) -> Result<(), InputError> {
        match *event {
            // Solo tiene sentido del lado de captura; no hay nada que inyectar.
            CapturedEvent::AbsolutePosition { .. } => Ok(()),
            CapturedEvent::MouseMove { x, y } => {
                let origin_x = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
                let origin_y = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
                let width = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
                let height = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };
                let dx = normalize_absolute(x, origin_x, width);
                let dy = normalize_absolute(y, origin_y, height);
                send_mouse(
                    dx,
                    dy,
                    0,
                    MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                )
            }
            CapturedEvent::MouseButton { button, pressed } => {
                let (flags, mouse_data) = match button {
                    MouseButton::Left if pressed => (MOUSEEVENTF_LEFTDOWN, 0),
                    MouseButton::Left => (MOUSEEVENTF_LEFTUP, 0),
                    MouseButton::Right if pressed => (MOUSEEVENTF_RIGHTDOWN, 0),
                    MouseButton::Right => (MOUSEEVENTF_RIGHTUP, 0),
                    MouseButton::Middle if pressed => (MOUSEEVENTF_MIDDLEDOWN, 0),
                    MouseButton::Middle => (MOUSEEVENTF_MIDDLEUP, 0),
                    MouseButton::Back if pressed => (MOUSEEVENTF_XDOWN, i32::from(XBUTTON1)),
                    MouseButton::Back => (MOUSEEVENTF_XUP, i32::from(XBUTTON1)),
                    MouseButton::Forward if pressed => (MOUSEEVENTF_XDOWN, i32::from(XBUTTON2)),
                    MouseButton::Forward => (MOUSEEVENTF_XUP, i32::from(XBUTTON2)),
                };
                send_mouse(0, 0, mouse_data, flags)
            }
            CapturedEvent::Key {
                keycode, pressed, ..
            } => {
                // NOTA: mismo hueco de normalización de keycodes que el
                // backend X11 — ver comentario en `x11::inject`.
                let vk = u16::try_from(keycode).map_err(|_| {
                    InputError::Windows(format!("keycode fuera de rango: {keycode}"))
                })?;
                send_key(vk, !pressed)
            }
        }
    }
}
