//! Prueba de humo contra un servidor X real. **No corre en CI/por defecto**
//! (`#[ignore]`): requiere `DISPLAY` apuntando a un servidor con las
//! extensiones XTEST y `XInput2` — usar un `Xephyr`/`Xvfb` anidado, nunca la
//! sesión real del desarrollador (esto mueve el cursor e inyecta teclas).
//!
//! Ejecutar manualmente, por ejemplo:
//! `Xephyr :5 -screen 640x480 -ac & DISPLAY=:5 cargo test -p ionconnect-input --test x11_smoke -- --ignored`

#![cfg(all(unix, not(target_os = "macos")))]

use std::sync::mpsc;
use std::time::Duration;

use ionconnect_input::x11::{X11Capture, X11Injector};
use ionconnect_input::{CapturedEvent, InputInjector};
use ionconnect_protocol::MouseButton;

#[test]
#[ignore = "requiere un servidor X real (Xephyr/Xvfb) en $DISPLAY"]
fn injects_mouse_move_and_click_without_error() {
    let mut injector = X11Injector::connect().expect("XTEST debería estar disponible");
    injector
        .inject(&CapturedEvent::MouseMove { x: 100, y: 50 })
        .expect("mover el mouse no debería fallar");
    injector
        .inject(&CapturedEvent::MouseButton {
            button: MouseButton::Left,
            pressed: true,
        })
        .expect("presionar el botón no debería fallar");
    injector
        .inject(&CapturedEvent::MouseButton {
            button: MouseButton::Left,
            pressed: false,
        })
        .expect("soltar el botón no debería fallar");
}

#[test]
#[ignore = "requiere un servidor X real (Xephyr/Xvfb) en $DISPLAY"]
fn capture_reports_injected_motion() {
    let (tx, rx) = mpsc::channel();
    let mut capture = X11Capture::connect(0, 0).expect("XInput2 debería estar disponible");

    let handle = std::thread::spawn(move || {
        use ionconnect_input::InputCapture as _;
        let _ = capture.run(tx);
    });

    // Dale tiempo al hilo de captura a registrarse antes de generar el evento.
    std::thread::sleep(Duration::from_millis(200));
    let mut injector = X11Injector::connect().expect("XTEST debería estar disponible");
    injector
        .inject(&CapturedEvent::MouseMove { x: 10, y: 10 })
        .expect("mover el mouse no debería fallar");

    let event = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("se esperaba recibir un evento de movimiento capturado");
    assert!(matches!(event, CapturedEvent::MouseMove { .. }));

    drop(handle);
}
