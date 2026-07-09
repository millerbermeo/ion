use ashpd::desktop::remote_desktop::{
    Axis, DeviceType, KeyState, NotifyKeyboardKeycodeOptions, NotifyPointerAxisDiscreteOptions,
    NotifyPointerButtonOptions, NotifyPointerMotionOptions, RemoteDesktop, SelectDevicesOptions,
    StartOptions,
};
use ashpd::desktop::{CreateSessionOptions, Session};
use ashpd::enumflags2::BitFlags;

use ionconnect_protocol::MouseButton;

use crate::error::InputError;
use crate::event::CapturedEvent;
use crate::inject::InputInjector;

fn portal_error(err: impl std::fmt::Display) -> InputError {
    InputError::Portal(err.to_string())
}

/// Códigos de botón Linux Evdev (los que el portal `RemoteDesktop` espera:
/// "encoded according to Linux Evdev button codes").
const BTN_LEFT: i32 = 0x110;
const BTN_RIGHT: i32 = 0x111;
const BTN_MIDDLE: i32 = 0x112;
const BTN_SIDE: i32 = 0x113;
const BTN_EXTRA: i32 = 0x114;

/// `None` para las variantes de scroll: el portal no las modela como botón
/// sino como eje discreto (ver [`scroll_axis`]) — se manejan aparte en
/// [`WaylandPortalInjector::inject_async`].
const fn button_to_evdev(button: MouseButton) -> Option<i32> {
    match button {
        MouseButton::Left => Some(BTN_LEFT),
        MouseButton::Right => Some(BTN_RIGHT),
        MouseButton::Middle => Some(BTN_MIDDLE),
        MouseButton::Back => Some(BTN_SIDE),
        MouseButton::Forward => Some(BTN_EXTRA),
        MouseButton::ScrollUp
        | MouseButton::ScrollDown
        | MouseButton::ScrollLeft
        | MouseButton::ScrollRight => None,
    }
}

/// Eje y cantidad de pasos para una muesca de scroll — convención estándar
/// wayland/libinput: vertical positivo = hacia abajo, horizontal positivo =
/// hacia la derecha.
const fn scroll_axis(button: MouseButton) -> Option<(Axis, i32)> {
    match button {
        MouseButton::ScrollUp => Some((Axis::Vertical, -1)),
        MouseButton::ScrollDown => Some((Axis::Vertical, 1)),
        MouseButton::ScrollLeft => Some((Axis::Horizontal, -1)),
        MouseButton::ScrollRight => Some((Axis::Horizontal, 1)),
        _ => None,
    }
}

/// Inyector de Wayland vía el portal `RemoteDesktop`. `NotifyPointerMotion`
/// del portal solo acepta deltas relativos (no hay posición absoluta sin
/// una sesión de `ScreenCast` asociada a un nodo `PipeWire`, que este backend
/// no negocia); por eso este inyector guarda la última posición absoluta
/// recibida y reenvía la diferencia.
pub struct WaylandPortalInjector {
    portal: RemoteDesktop,
    session: Session<RemoteDesktop>,
    last_position: Option<(f64, f64)>,
}

impl WaylandPortalInjector {
    /// Negocia una sesión `RemoteDesktop` con acceso a puntero y teclado.
    /// Esto típicamente dispara un diálogo de permiso del compositor la
    /// primera vez.
    ///
    /// # Errors
    ///
    /// Devuelve [`InputError::Unsupported`] si el portal no está disponible
    /// o el usuario rechaza el permiso.
    pub async fn connect() -> Result<Self, InputError> {
        let portal = RemoteDesktop::new().await.map_err(portal_error)?;
        let session = portal
            .create_session(CreateSessionOptions::default())
            .await
            .map_err(portal_error)?;

        portal
            .select_devices(
                &session,
                SelectDevicesOptions::default()
                    .set_devices(BitFlags::from(DeviceType::Pointer) | DeviceType::Keyboard),
            )
            .await
            .map_err(portal_error)?
            .response()
            .map_err(portal_error)?;

        portal
            .start(&session, None, StartOptions::default())
            .await
            .map_err(portal_error)?
            .response()
            .map_err(portal_error)?;

        Ok(Self {
            portal,
            session,
            last_position: None,
        })
    }

    async fn inject_async(&mut self, event: &CapturedEvent) -> Result<(), InputError> {
        match *event {
            // Solo tiene sentido del lado de captura; no hay nada que inyectar.
            CapturedEvent::AbsolutePosition { .. } => Ok(()),
            CapturedEvent::MouseMove { x, y } => {
                let (x, y) = (f64::from(x), f64::from(y));
                let (dx, dy) = match self.last_position {
                    Some((last_x, last_y)) => (x - last_x, y - last_y),
                    None => (0.0, 0.0),
                };
                self.last_position = Some((x, y));
                self.portal
                    .notify_pointer_motion(
                        &self.session,
                        dx,
                        dy,
                        NotifyPointerMotionOptions::default(),
                    )
                    .await
                    .map_err(portal_error)
            }
            // El scroll no es un botón que se sostiene sino una muesca
            // discreta — el emisor la reporta como un par press+release
            // instantáneo (ver `x11::util::button_to_code`), así que basta
            // con actuar en el `pressed: true` e ignorar el `false` que le
            // sigue, igual que hace `win32::inject`.
            CapturedEvent::MouseButton { button, pressed } if scroll_axis(button).is_some() => {
                let (axis, steps) =
                    scroll_axis(button).expect("scroll_axis(button).is_some() ya comprobado");
                if !pressed {
                    return Ok(());
                }
                self.portal
                    .notify_pointer_axis_discrete(
                        &self.session,
                        axis,
                        steps,
                        NotifyPointerAxisDiscreteOptions::default(),
                    )
                    .await
                    .map_err(portal_error)
            }
            CapturedEvent::MouseButton { button, pressed } => {
                let state = if pressed {
                    KeyState::Pressed
                } else {
                    KeyState::Released
                };
                let Some(evdev) = button_to_evdev(button) else {
                    return Ok(());
                };
                self.portal
                    .notify_pointer_button(
                        &self.session,
                        evdev,
                        state,
                        NotifyPointerButtonOptions::default(),
                    )
                    .await
                    .map_err(portal_error)
            }
            CapturedEvent::Key {
                keycode, pressed, ..
            } => {
                // NOTA: mismo hueco de normalización de keycodes que los
                // backends X11/Win32 — ver comentario en `x11::inject`.
                let state = if pressed {
                    KeyState::Pressed
                } else {
                    KeyState::Released
                };
                let keycode = i32::try_from(keycode).map_err(|_| {
                    InputError::Unsupported("keycode fuera de rango para el portal")
                })?;
                self.portal
                    .notify_keyboard_keycode(
                        &self.session,
                        keycode,
                        state,
                        NotifyKeyboardKeycodeOptions::default(),
                    )
                    .await
                    .map_err(portal_error)
            }
        }
    }
}

impl InputInjector for WaylandPortalInjector {
    /// Bloquea el hilo actual sobre un runtime de tokio para ejecutar la
    /// llamada D-Bus async subyacente. Este backend está pensado para
    /// llamarse desde `tokio::task::spawn_blocking`, igual que los backends
    /// síncronos de X11/Windows, así el resto de `core` no necesita saber
    /// que este camino en particular es async por dentro.
    fn inject(&mut self, event: &CapturedEvent) -> Result<(), InputError> {
        tokio::runtime::Handle::current().block_on(self.inject_async(event))
    }
}
