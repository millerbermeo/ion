use std::os::unix::net::UnixStream;

use ashpd::desktop::input_capture::{
    Barrier, BarrierID, Capabilities, ConnectToEISOptions, CreateSessionOptions, EnableOptions,
    GetZonesOptions, InputCapture as PortalInputCapture, ReleaseOptions, SetPointerBarriersOptions,
    StartOptions,
};
use ashpd::desktop::Session;
use futures_util::{Stream, StreamExt};
use reis::ei;
use reis::event::{DeviceCapability, EiEvent};
use reis::tokio::EiConvertEventStream;

use ionconnect_shared::KeyModifiers;

use ionconnect_protocol::MouseButton;

use crate::error::InputError;
use crate::event::CapturedEvent;

fn portal_error(err: impl std::fmt::Display) -> InputError {
    InputError::Portal(err.to_string())
}

/// Códigos de botón Linux Evdev — lo que reporta `libei` en
/// [`reis::event::Button::button`]. Misma convención que
/// `wayland::inject::button_to_evdev`, en sentido inverso.
const fn button_from_evdev(code: u32) -> Option<MouseButton> {
    match code {
        0x110 => Some(MouseButton::Left),
        0x111 => Some(MouseButton::Right),
        0x112 => Some(MouseButton::Middle),
        0x113 => Some(MouseButton::Back),
        0x114 => Some(MouseButton::Forward),
        _ => None,
    }
}

/// Una de las pantallas (salidas del compositor) que el portal reporta como
/// parte del escritorio capturable — equivalente Wayland de
/// [`ionconnect_screen::MonitorGeometry`], pero reportada por el
/// compositor en vez de calculada a mano.
#[derive(Debug, Clone, Copy)]
pub struct CaptureZone {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Una barrera a lo largo de un borde de pantalla — cuando el cursor la
/// cruza, el compositor activa la captura. `id` debe ser distinto de cero
/// y único dentro de la sesión.
#[derive(Debug, Clone, Copy)]
pub struct BarrierSpec {
    pub id: u32,
    pub x1: i32,
    pub y1: i32,
    pub x2: i32,
    pub y2: i32,
}

/// Lo que puede reportar una sesión de captura Wayland: activación de un
/// borde (con el `barrier_id` que se configuró para él, así el llamador
/// sabe a qué vecino corresponde), desactivación, o un evento de
/// entrada ya traducido mientras la captura está activa.
#[derive(Debug)]
pub enum WaylandCaptureEvent {
    Activated {
        activation_id: Option<u32>,
        barrier_id: Option<u32>,
        cursor: Option<(f32, f32)>,
    },
    Deactivated {
        activation_id: Option<u32>,
    },
    Input(CapturedEvent),
}

/// Sesión de captura de entrada vía el portal `InputCapture`
/// (`org.freedesktop.portal.InputCapture` + protocolo `libei`/EIS) —
/// equivalente Wayland de [`super::super::x11::X11Capture`] +
/// [`super::super::x11::X11Control`] combinados, porque en este modelo el
/// propio compositor decide cuándo se cruzó un borde (vía barreras) en vez
/// de que nosotros sondeemos posición contra una geometría propia.
pub struct WaylandCaptureSession {
    portal: PortalInputCapture,
    session: Session<PortalInputCapture>,
    ei_context: ei::Context,
    events: EiConvertEventStream,
    /// Posición acumulada a partir de los deltas de `PointerMotion` —
    /// mientras la captura está activa es lo único que llega (relativo,
    /// como el `RawMotion` de X11), así que hay que integrarlo nosotros.
    /// Reiniciar con [`Self::reset_position`] en cada activación, al punto
    /// de entrada calculado por `screen::Layout::detect_crossing`.
    position: (i32, i32),
}

impl WaylandCaptureSession {
    /// Negocia una sesión de captura con el compositor. Dispara un diálogo
    /// de permiso la primera vez que corre este equipo.
    ///
    /// # Errors
    ///
    /// Devuelve [`InputError::Portal`] si el portal no está disponible, la
    /// negociación EIS falla, o el usuario rechaza el permiso.
    pub async fn connect() -> Result<Self, InputError> {
        let portal = PortalInputCapture::new().await.map_err(portal_error)?;
        let capabilities = Capabilities::Keyboard | Capabilities::Pointer;

        let session = match portal.create_session2(Default::default()).await {
            Ok(session) => {
                portal
                    .start(
                        &session,
                        None,
                        StartOptions::default().set_capabilities(capabilities),
                    )
                    .await
                    .map_err(portal_error)?
                    .response()
                    .map_err(portal_error)?;
                session
            }
            Err(ashpd::Error::RequiresVersion(_, _)) => {
                let (session, _capabilities) = portal
                    .create_session(
                        None,
                        CreateSessionOptions::default().set_capabilities(capabilities),
                    )
                    .await
                    .map_err(portal_error)?;
                session
            }
            Err(err) => return Err(portal_error(err)),
        };

        let fd = portal
            .connect_to_eis(&session, ConnectToEISOptions::default())
            .await
            .map_err(portal_error)?;
        let stream = UnixStream::from(fd);
        let ei_context = ei::Context::new(stream).map_err(|e| InputError::Portal(e.to_string()))?;
        let (_connection, events) = ei_context
            .handshake_tokio("ionconnect", ei::handshake::ContextType::Receiver)
            .await
            .map_err(portal_error)?;

        Ok(Self {
            portal,
            session,
            ei_context,
            events,
            position: (0, 0),
        })
    }

    /// Reinicia la posición acumulada — llamar exactamente en el momento de
    /// una activación, con el punto de entrada calculado por
    /// `screen::Layout::detect_crossing`.
    pub fn reset_position(&mut self, x: i32, y: i32) {
        self.position = (x, y);
    }

    /// Última posición acumulada — para estampar coordenadas en eventos de
    /// botón/tecla, que EIS no trae con posición propia.
    #[must_use]
    pub fn position(&self) -> (i32, i32) {
        self.position
    }

    /// Pantallas disponibles para configurar barreras, tal como las ve el
    /// compositor — ya viene con la geometría multi-monitor correcta, sin
    /// necesidad de consultarla aparte.
    ///
    /// # Errors
    ///
    /// Devuelve [`InputError::Portal`] si la solicitud al portal falla.
    pub async fn zones(&self) -> Result<(Vec<CaptureZone>, u32), InputError> {
        let response = self
            .portal
            .zones(&self.session, GetZonesOptions::default())
            .await
            .map_err(portal_error)?
            .response()
            .map_err(portal_error)?;
        let zones = response
            .regions()
            .iter()
            .map(|r| CaptureZone {
                x: r.x_offset(),
                y: r.y_offset(),
                width: r.width(),
                height: r.height(),
            })
            .collect();
        Ok((zones, response.zone_set()))
    }

    /// Registra las barreras que van a disparar la captura. Devuelve los
    /// `id` de las que el compositor rechazó (posición inválida — p. ej.
    /// entre dos monitores, no en el borde exterior).
    ///
    /// # Errors
    ///
    /// Devuelve [`InputError::Portal`] si la solicitud al portal falla.
    pub async fn set_barriers(&self, barriers: &[BarrierSpec]) -> Result<Vec<u32>, InputError> {
        let (_zones, zone_set) = self.zones().await?;
        let portal_barriers: Vec<Barrier> = barriers
            .iter()
            .filter_map(|b| {
                let id = BarrierID::new(b.id)?;
                Some(Barrier::new(id, (b.x1, b.y1, b.x2, b.y2)))
            })
            .collect();

        let response = self
            .portal
            .set_pointer_barriers(
                &self.session,
                &portal_barriers,
                zone_set,
                SetPointerBarriersOptions::default(),
            )
            .await
            .map_err(portal_error)?
            .response()
            .map_err(portal_error)?;
        Ok(response
            .failed_barriers()
            .iter()
            .map(|id| id.get())
            .collect())
    }

    /// Habilita la captura — a partir de acá, cruzar una barrera configurada
    /// dispara `Activated`.
    ///
    /// # Errors
    ///
    /// Devuelve [`InputError::Portal`] si la solicitud al portal falla.
    pub async fn enable(&self) -> Result<(), InputError> {
        self.portal
            .enable(&self.session, EnableOptions::default())
            .await
            .map_err(portal_error)
    }

    /// Devuelve el control local, reapareciendo el cursor en `cursor` si se
    /// indica.
    ///
    /// # Errors
    ///
    /// Devuelve [`InputError::Portal`] si la solicitud al portal falla.
    pub async fn release(
        &self,
        activation_id: Option<u32>,
        cursor: Option<(f64, f64)>,
    ) -> Result<(), InputError> {
        self.portal
            .release(
                &self.session,
                ReleaseOptions::default()
                    .set_activation_id(activation_id)
                    .set_cursor_position(cursor),
            )
            .await
            .map_err(portal_error)
    }

    /// Espera el próximo evento relevante: activación/desactivación de la
    /// captura, o un evento de entrada ya traducido (solo llegan estos
    /// últimos mientras la captura está activa). Maneja transparentemente
    /// el registro de capacidades del `seat` (`SeatAdded`) — el llamador
    /// nunca ve esos eventos de bajo nivel.
    ///
    /// # Errors
    ///
    /// Devuelve [`InputError::Portal`] si el stream de eventos del portal o
    /// de EIS termina con error.
    pub async fn next_event(
        &mut self,
        activated: &mut (impl Stream<Item = ashpd::desktop::input_capture::Activated> + Unpin),
        deactivated: &mut (impl Stream<Item = ashpd::desktop::input_capture::Deactivated> + Unpin),
    ) -> Result<WaylandCaptureEvent, InputError> {
        loop {
            tokio::select! {
                activated = activated.next() => {
                    let activated = activated.ok_or_else(|| {
                        InputError::Portal("el stream de activación del portal terminó".to_string())
                    })?;
                    let barrier_id = activated.barrier_id().and_then(|b| match b {
                        ashpd::desktop::input_capture::ActivatedBarrier::Barrier(id) => Some(id.get()),
                        ashpd::desktop::input_capture::ActivatedBarrier::UnknownBarrier => None,
                    });
                    return Ok(WaylandCaptureEvent::Activated {
                        activation_id: activated.activation_id(),
                        barrier_id,
                        cursor: activated.cursor_position(),
                    });
                }
                deactivated = deactivated.next() => {
                    let deactivated = deactivated.ok_or_else(|| {
                        InputError::Portal("el stream de desactivación del portal terminó".to_string())
                    })?;
                    return Ok(WaylandCaptureEvent::Deactivated {
                        activation_id: deactivated.activation_id(),
                    });
                }
                event = self.events.next() => {
                    let event = event.ok_or_else(|| {
                        InputError::Portal("el stream de eventos EIS terminó".to_string())
                    })?.map_err(portal_error)?;
                    if let Some(captured) = self.handle_ei_event(event) {
                        return Ok(WaylandCaptureEvent::Input(captured));
                    }
                    // `SeatAdded`/`DeviceAdded`/`Frame`/etc. no se traducen a
                    // ningún `CapturedEvent` directamente — seguimos
                    // esperando el próximo evento relevante.
                }
            }
        }
    }

    /// Handle independiente para suscribirse a las señales
    /// `Activated`/`Deactivated` — deliberadamente **no** un método de
    /// `&self` que devuelva el stream directamente: ese stream necesita
    /// vivir durante todo el bucle principal al mismo tiempo que hace
    /// falta `&mut self` para leer eventos EIS y reiniciar posición: dos
    /// préstamos que no pueden coexistir si ambos cuelgan del mismo
    /// `self`. Un [`ActivationWatcher`] separado, sobre la misma conexión
    /// D-Bus (no abre una conexión nueva), no tiene ese problema porque es
    /// una variable propia en el llamador.
    ///
    /// # Errors
    ///
    /// Devuelve [`InputError::Portal`] si no se pudo crear el proxy.
    pub async fn activation_watcher(&self) -> Result<ActivationWatcher, InputError> {
        let portal = PortalInputCapture::with_connection(self.portal.connection().clone())
            .await
            .map_err(portal_error)?;
        Ok(ActivationWatcher { portal })
    }

    fn handle_ei_event(&mut self, event: EiEvent) -> Option<CapturedEvent> {
        match event {
            EiEvent::SeatAdded(seat_event) => {
                seat_event.seat.bind_capabilities(
                    DeviceCapability::Pointer
                        | DeviceCapability::PointerAbsolute
                        | DeviceCapability::Keyboard
                        | DeviceCapability::Scroll
                        | DeviceCapability::Button,
                );
                let _ = self.ei_context.flush();
                None
            }
            EiEvent::PointerMotion(m) => {
                self.position.0 = add_delta(self.position.0, f64::from(m.dx));
                self.position.1 = add_delta(self.position.1, f64::from(m.dy));
                Some(CapturedEvent::MouseMove {
                    x: self.position.0,
                    y: self.position.1,
                })
            }
            EiEvent::Button(b) => button_from_evdev(b.button).map(|button| {
                CapturedEvent::MouseButton {
                    button,
                    pressed: matches!(b.state, ei::button::ButtonState::Press),
                }
            }),
            EiEvent::KeyboardKey(k) => Some(CapturedEvent::Key {
                keycode: k.key,
                modifiers: KeyModifiers::NONE,
                pressed: matches!(k.state, ei::keyboard::KeyState::Press),
            }),
            _ => None,
        }
    }
}

/// Trunca un delta de `PointerMotion` a un rango razonable antes de
/// sumarlo a la posición acumulada — mismo motivo que la versión X11 en
/// `x11::capture`: un evento nunca debería mover miles de píxeles de
/// golpe, así que saturar es más seguro que un `as i32` silencioso.
#[allow(clippy::cast_possible_truncation)]
fn add_delta(base: i32, delta: f64) -> i32 {
    let delta = delta.round().clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
    base.saturating_add(delta)
}

/// Handle separado de [`WaylandCaptureSession`] para suscribirse a las
/// señales `Activated`/`Deactivated` sin pelearse por el préstamo de
/// `&mut self` de la sesión — ver [`WaylandCaptureSession::activation_watcher`].
pub struct ActivationWatcher {
    portal: PortalInputCapture,
}

impl ActivationWatcher {
    /// Stream de señales `Activated` — pasar a [`WaylandCaptureSession::next_event`].
    ///
    /// # Errors
    ///
    /// Devuelve [`InputError::Portal`] si no se pudo suscribir a la señal.
    pub async fn receive_activated(
        &self,
    ) -> Result<impl Stream<Item = ashpd::desktop::input_capture::Activated> + use<'_>, InputError>
    {
        self.portal.receive_activated().await.map_err(portal_error)
    }

    /// Stream de señales `Deactivated` — pasar a [`WaylandCaptureSession::next_event`].
    ///
    /// # Errors
    ///
    /// Devuelve [`InputError::Portal`] si no se pudo suscribir a la señal.
    pub async fn receive_deactivated(
        &self,
    ) -> Result<impl Stream<Item = ashpd::desktop::input_capture::Deactivated> + use<'_>, InputError>
    {
        self.portal
            .receive_deactivated()
            .await
            .map_err(portal_error)
    }
}
