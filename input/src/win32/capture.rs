use std::cell::RefCell;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::Sender;

use ionconnect_shared::KeyModifiers;
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT,
    PostThreadMessageW, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL,
    WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP,
    WM_MOUSEMOVE, WM_QUIT, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
    WM_XBUTTONDOWN, WM_XBUTTONUP,
};

use ionconnect_protocol::MouseButton;

use crate::capture::InputCapture;
use crate::error::InputError;
use crate::event::CapturedEvent;

thread_local! {
    /// Los hooks de bajo nivel de Windows no permiten pasar un puntero de
    /// usuario al callback; se usa almacenamiento thread-local del propio
    /// hilo que instala el hook (que es el mismo que corre el bucle de
    /// mensajes) para entregarle el `Sender`.
    static SINK: RefCell<Option<Sender<CapturedEvent>>> = const { RefCell::new(None) };
}

/// Id del hilo que corre el bucle de mensajes, para poder inyectarle
/// `WM_QUIT` desde `stop()` y así desbloquear `GetMessageW`.
static CAPTURE_THREAD_ID: AtomicU32 = AtomicU32::new(0);

/// Captura de Windows vía `SetWindowsHookEx` (`WH_MOUSE_LL` +
/// `WH_KEYBOARD_LL`). `run` instala los hooks y bombea el bucle de mensajes
/// en el hilo llamador hasta `WM_QUIT`.
#[derive(Debug, Default)]
pub struct WindowsCapture;

impl WindowsCapture {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl InputCapture for WindowsCapture {
    fn run(&mut self, sink: Sender<CapturedEvent>) -> Result<(), InputError> {
        SINK.with(|cell| *cell.borrow_mut() = Some(sink));
        CAPTURE_THREAD_ID.store(unsafe { GetCurrentThreadId() }, Ordering::SeqCst);

        let mouse_hook = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), None, 0) }
            .map_err(|e| InputError::Windows(e.to_string()))?;
        let keyboard_hook =
            unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), None, 0) }
                .map_err(|e| InputError::Windows(e.to_string()))?;

        let result = pump_messages();

        unsafe {
            let _ = UnhookWindowsHookEx(mouse_hook);
            let _ = UnhookWindowsHookEx(keyboard_hook);
        }
        SINK.with(|cell| *cell.borrow_mut() = None);

        result
    }

    fn stop(&mut self) {
        let thread_id = CAPTURE_THREAD_ID.load(Ordering::SeqCst);
        if thread_id != 0 {
            unsafe {
                let _ = PostThreadMessageW(thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        }
    }
}

fn pump_messages() -> Result<(), InputError> {
    let mut msg = MSG::default();
    loop {
        let ok = unsafe { GetMessageW(&mut msg, None, 0, 0) }.0;
        if ok <= 0 {
            break;
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}

fn emit(event: CapturedEvent) {
    SINK.with(|cell| {
        if let Some(sender) = cell.borrow().as_ref() {
            let _ = sender.send(event);
        }
    });
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let data = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        let x = data.pt.x;
        let y = data.pt.y;
        match wparam.0 as u32 {
            WM_MOUSEMOVE => emit(CapturedEvent::MouseMove { x, y }),
            WM_LBUTTONDOWN => emit(CapturedEvent::MouseButton {
                button: MouseButton::Left,
                pressed: true,
            }),
            WM_LBUTTONUP => emit(CapturedEvent::MouseButton {
                button: MouseButton::Left,
                pressed: false,
            }),
            WM_RBUTTONDOWN => emit(CapturedEvent::MouseButton {
                button: MouseButton::Right,
                pressed: true,
            }),
            WM_RBUTTONUP => emit(CapturedEvent::MouseButton {
                button: MouseButton::Right,
                pressed: false,
            }),
            WM_MBUTTONDOWN => emit(CapturedEvent::MouseButton {
                button: MouseButton::Middle,
                pressed: true,
            }),
            WM_MBUTTONUP => emit(CapturedEvent::MouseButton {
                button: MouseButton::Middle,
                pressed: false,
            }),
            WM_XBUTTONDOWN | WM_XBUTTONUP => {
                // El botón X (back/forward) viaja en el high word de mouseData.
                let x_button = (data.mouseData >> 16) & 0xFFFF;
                let button = if x_button == 1 {
                    MouseButton::Back
                } else {
                    MouseButton::Forward
                };
                emit(CapturedEvent::MouseButton {
                    button,
                    pressed: wparam.0 as u32 == WM_XBUTTONDOWN,
                });
            }
            _ => {}
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let data = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let pressed = matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
        let released = matches!(wparam.0 as u32, WM_KEYUP | WM_SYSKEYUP);
        if pressed || released {
            // NOTA: mismo hueco de normalización de keycodes que en X11 —
            // `vkCode` es el virtual-key nativo de Windows, sin traducir a
            // un espacio de keycodes común entre sistemas operativos.
            emit(CapturedEvent::Key {
                keycode: data.vkCode,
                modifiers: KeyModifiers::NONE,
                pressed,
            });
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}
