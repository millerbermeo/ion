use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP, MOUSEEVENTF_ABSOLUTE,
    MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN,
    MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
    MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEINPUT,
    SendInput, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    WHEEL_DELTA, XBUTTON1, XBUTTON2,
};

use ionconnect_protocol::MouseButton;

use crate::error::InputError;
use crate::event::CapturedEvent;
use crate::inject::InputInjector;
use crate::win32::keymap::evdev_to_vk;

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

/// Origen y tamaño del escritorio virtual completo (todos los monitores) —
/// el tamaño es lo que usa `normalize_absolute` para mapear coordenadas a la
/// escala `MOUSEEVENTF_ABSOLUTE`, y lo que reporta este equipo como cliente al
/// conectarse (ver `core::client`) para que el servidor deje de asumir
/// que tiene su misma resolución.
#[must_use]
pub fn virtual_screen_geometry() -> (i32, i32, i32, i32) {
    unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    }
}

/// Convierte una coordenada a la escala 0..=65535 que
/// `MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK` espera.
///
/// `offset` ya es relativo a la esquina superior izquierda del escritorio
/// virtual, **no** una coordenada de pantalla de Windows: el servidor solo
/// conoce el ancho/alto que reportamos (`DisplayGeometry`) y modela nuestro
/// escritorio como `0..width`. En Windows ese escritorio empieza en
/// coordenadas negativas cuando hay monitores a la izquierda o arriba del
/// principal (p. ej. `x = -1920`). Antes se restaba ese origen otra vez, así
/// que todo quedaba corrido: el cursor entraba en el monitor principal en vez
/// del borde real, el monitor de la izquierda era inalcanzable y el borde de
/// vuelta al servidor quedaba en el medio. Solo funcionaba con el principal
/// a la izquierda (origen 0).
fn normalize_absolute(offset: i32, extent: i32) -> i32 {
    if extent <= 0 {
        return 0;
    }
    let offset = offset.clamp(0, extent - 1);
    ((i64::from(offset) * 65535) / i64::from(extent)) as i32
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
                let (_, _, width, height) = virtual_screen_geometry();
                let dx = normalize_absolute(x, width);
                let dy = normalize_absolute(y, height);
                send_mouse(
                    dx,
                    dy,
                    0,
                    MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                )
            }
            // El scroll wheel de Windows no tiene down/up: `SendInput` con
            // `MOUSEEVENTF_WHEEL`/`_HWHEEL` es un único evento atómico por
            // muesca. El emisor (ver `x11::util::button_to_code`) reporta
            // cada muesca como un par press+release — acá solo se actúa en
            // el `pressed: true` y se ignora el `false` que le sigue.
            CapturedEvent::MouseButton {
                button:
                    button @ (MouseButton::ScrollUp
                    | MouseButton::ScrollDown
                    | MouseButton::ScrollLeft
                    | MouseButton::ScrollRight),
                pressed,
            } => {
                if !pressed {
                    return Ok(());
                }
                #[allow(clippy::cast_possible_wrap)]
                let delta = WHEEL_DELTA as i32;
                let (flags, mouse_data) = match button {
                    MouseButton::ScrollUp => (MOUSEEVENTF_WHEEL, delta),
                    MouseButton::ScrollDown => (MOUSEEVENTF_WHEEL, -delta),
                    MouseButton::ScrollRight => (MOUSEEVENTF_HWHEEL, delta),
                    MouseButton::ScrollLeft => (MOUSEEVENTF_HWHEEL, -delta),
                    MouseButton::Left
                    | MouseButton::Right
                    | MouseButton::Middle
                    | MouseButton::Back
                    | MouseButton::Forward => unreachable!("filtrado por el patrón del match"),
                };
                send_mouse(0, 0, mouse_data, flags)
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
                    MouseButton::ScrollUp
                    | MouseButton::ScrollDown
                    | MouseButton::ScrollLeft
                    | MouseButton::ScrollRight => {
                        unreachable!("filtrado por el brazo anterior del match")
                    }
                };
                send_mouse(0, 0, mouse_data, flags)
            }
            CapturedEvent::Key {
                keycode, pressed, ..
            } => {
                // `keycode` viaja como keycode `evdev` (normalizado por el
                // emisor, sea X11 o Wayland) — hace falta traducirlo al
                // espacio de `VIRTUAL_KEY` de Windows, no son el mismo
                // número. Sin mapeo conocido, no inyecta nada en vez de
                // mandar una tecla equivocada.
                let Some(vk) = evdev_to_vk(keycode) else {
                    return Ok(());
                };
                send_key(vk.0, !pressed)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_absolute;

    #[test]
    fn normalize_is_relative_to_virtual_desktop_origin() {
        // Escritorio de 3840px (monitor a la izquierda del principal): el
        // servidor manda 0..3840, sin importar que Windows lo ubique en -1920.
        assert_eq!(normalize_absolute(0, 3840), 0);
        assert_eq!(normalize_absolute(1920, 3840), 32767);
        assert!(normalize_absolute(3839, 3840) > 65500);
    }

    #[test]
    fn normalize_clamps_out_of_range() {
        assert_eq!(normalize_absolute(-50, 1920), 0);
        assert_eq!(normalize_absolute(5000, 1920), normalize_absolute(1919, 1920));
    }
}
