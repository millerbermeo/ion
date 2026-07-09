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

use ionconnect_input::x11::{SharedPosition, X11Capture, X11Control, X11Injector};
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
    let position = SharedPosition::new(0, 0);
    let mut capture = X11Capture::connect(position).expect("XInput2 debería estar disponible");

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

    // El servidor puede entregar primero el evento no crudo (posición
    // absoluta) y luego el crudo (delta acumulado), o viceversa; alcanza
    // con que aparezca alguno de movimiento dentro de la ventana de tiempo.
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let mut saw_motion = false;
    while std::time::Instant::now() < deadline {
        let Ok(event) =
            rx.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
        else {
            break;
        };
        if matches!(
            event,
            CapturedEvent::MouseMove { .. } | CapturedEvent::AbsolutePosition { .. }
        ) {
            saw_motion = true;
            break;
        }
    }
    assert!(
        saw_motion,
        "se esperaba recibir algún evento de movimiento capturado"
    );

    drop(handle);
}

#[test]
#[ignore = "requiere un servidor X real (Xephyr/Xvfb) en $DISPLAY"]
fn grab_ungrab_and_warp_do_not_error() {
    let control = X11Control::connect().expect("la conexión de control no debería fallar");

    control
        .grab(50, 60)
        .expect("agarrar puntero+teclado no debería fallar");
    control
        .warp_to(50, 60)
        .expect("mover el cursor real durante el grab no debería fallar");
    control
        .ungrab()
        .expect("soltar el agarre no debería fallar");

    // Tras soltar, el sistema debería volver a aceptar inyección normal.
    let mut injector = X11Injector::connect().expect("XTEST debería estar disponible");
    injector
        .inject(&CapturedEvent::MouseMove { x: 5, y: 5 })
        .expect("inyectar después de ungrab no debería fallar");
}

#[test]
#[ignore = "requiere un servidor X real (Xephyr/Xvfb) en $DISPLAY"]
fn capture_reports_a_single_key_press_and_release() {
    let (tx, rx) = mpsc::channel();
    let position = SharedPosition::new(0, 0);
    let mut capture = X11Capture::connect(position).expect("XInput2 debería estar disponible");

    let handle = std::thread::spawn(move || {
        use ionconnect_input::InputCapture as _;
        let _ = capture.run(tx);
    });

    std::thread::sleep(Duration::from_millis(200));
    let mut injector = X11Injector::connect().expect("XTEST debería estar disponible");
    // Keycode evdev 30 = 'a' (ver `x11_keycode_to_evdev`/`X11Injector::inject`,
    // que le suma/resta el offset de 8 que usa XKB).
    injector
        .inject(&CapturedEvent::Key {
            keycode: 30,
            modifiers: ionconnect_shared::KeyModifiers::NONE,
            pressed: true,
        })
        .expect("presionar la tecla no debería fallar");
    injector
        .inject(&CapturedEvent::Key {
            keycode: 30,
            modifiers: ionconnect_shared::KeyModifiers::NONE,
            pressed: false,
        })
        .expect("soltar la tecla no debería fallar");

    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let mut presses = 0u32;
    let mut releases = 0u32;
    while std::time::Instant::now() < deadline {
        let Ok(event) =
            rx.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
        else {
            break;
        };
        match event {
            CapturedEvent::Key {
                keycode: 30,
                pressed: true,
                ..
            } => presses += 1,
            CapturedEvent::Key {
                keycode: 30,
                pressed: false,
                ..
            } => releases += 1,
            _ => {}
        }
    }

    drop(handle);
    assert_eq!(presses, 1, "una sola tecla presionada no debería reportarse más de una vez");
    assert_eq!(releases, 1, "una sola tecla soltada no debería reportarse más de una vez");
}

#[test]
#[ignore = "requiere un servidor X real (Xephyr/Xvfb) en $DISPLAY"]
fn capture_reports_a_scroll_notch_once() {
    let (tx, rx) = mpsc::channel();
    let position = SharedPosition::new(0, 0);
    let mut capture = X11Capture::connect(position).expect("XInput2 debería estar disponible");

    let handle = std::thread::spawn(move || {
        use ionconnect_input::InputCapture as _;
        let _ = capture.run(tx);
    });

    std::thread::sleep(Duration::from_millis(200));
    let mut injector = X11Injector::connect().expect("XTEST debería estar disponible");
    // Una muesca de scroll es un click+release del botón 4 (arriba) —
    // ver `x11::util::button_to_code`.
    injector
        .inject(&CapturedEvent::MouseButton {
            button: MouseButton::ScrollUp,
            pressed: true,
        })
        .expect("la muesca de scroll no debería fallar al inyectarse");
    injector
        .inject(&CapturedEvent::MouseButton {
            button: MouseButton::ScrollUp,
            pressed: false,
        })
        .expect("la muesca de scroll no debería fallar al inyectarse");

    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let mut presses = 0u32;
    let mut releases = 0u32;
    while std::time::Instant::now() < deadline {
        let Ok(event) =
            rx.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
        else {
            break;
        };
        match event {
            CapturedEvent::MouseButton {
                button: MouseButton::ScrollUp,
                pressed: true,
            } => presses += 1,
            CapturedEvent::MouseButton {
                button: MouseButton::ScrollUp,
                pressed: false,
            } => releases += 1,
            _ => {}
        }
    }

    drop(handle);
    assert_eq!(presses, 1, "una sola muesca de scroll no debería reportarse más de una vez");
    assert_eq!(releases, 1, "una sola muesca de scroll no debería reportarse más de una vez");
}
