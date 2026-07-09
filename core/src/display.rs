//! Detecta, una sola vez al arrancar como servidor, con qué backend de
//! captura de entrada contamos (X11 nativo o el portal `InputCapture` de
//! Wayland) y qué geometría de pantalla real corresponde usar para el
//! `Layout` de hand-off.
//!
//! Se resuelve acá (no en `server::run_server`) porque conectar el backend
//! Wayland es async y puede disparar un diálogo de permiso — mejor
//! hacerlo una vez, antes de construir nada más, que reconectar más tarde
//! dentro de `spawn_input_session` y arriesgarse a pedir el permiso dos
//! veces.

use ionconnect_screen::MonitorGeometry;
use tracing::warn;

/// Backend real de captura a usar. `Unsupported` dejar el servidor
/// escuchando y autenticando peers con normalidad, pero sin nada que
/// capturar ni reenviar (mismo comportamiento que antes en plataformas sin
/// backend, o si Wayland rechazó el permiso).
pub enum CaptureBackend {
    #[cfg(all(unix, not(target_os = "macos")))]
    X11,
    #[cfg(all(unix, not(target_os = "macos")))]
    Wayland(ionconnect_input::wayland::WaylandCaptureSession),
    Unsupported,
}

pub struct LocalDisplay {
    pub geometry: MonitorGeometry,
    pub backend: CaptureBackend,
}

const FALLBACK_GEOMETRY: MonitorGeometry = MonitorGeometry::new(0, 0, 1920, 1080);

#[cfg(all(unix, not(target_os = "macos")))]
fn bounding_box(zones: &[ionconnect_input::wayland::CaptureZone]) -> Option<MonitorGeometry> {
    let mut iter = zones.iter();
    let first = iter.next()?;
    let mut left = first.x;
    let mut top = first.y;
    #[allow(clippy::cast_possible_wrap)]
    let mut right = first.x + first.width as i32;
    #[allow(clippy::cast_possible_wrap)]
    let mut bottom = first.y + first.height as i32;
    for zone in iter {
        left = left.min(zone.x);
        top = top.min(zone.y);
        #[allow(clippy::cast_possible_wrap)]
        {
            right = right.max(zone.x + zone.width as i32);
            bottom = bottom.max(zone.y + zone.height as i32);
        }
    }
    #[allow(clippy::cast_sign_loss)]
    Some(MonitorGeometry::new(
        left,
        top,
        (right - left) as u32,
        (bottom - top) as u32,
    ))
}

#[cfg(all(unix, not(target_os = "macos")))]
pub async fn detect_local_display() -> LocalDisplay {
    let session_type = std::env::var("XDG_SESSION_TYPE").unwrap_or_default();

    if session_type == "wayland" {
        return detect_wayland_display().await;
    }

    match ionconnect_input::x11::X11Control::root_geometry() {
        Ok((width, height)) => LocalDisplay {
            geometry: MonitorGeometry::new(0, 0, width, height),
            backend: CaptureBackend::X11,
        },
        Err(err) => {
            warn!(
                %err,
                "no se pudo consultar la geometría real de pantalla X11, usando 1920x1080 por defecto"
            );
            LocalDisplay {
                geometry: FALLBACK_GEOMETRY,
                backend: CaptureBackend::X11,
            }
        }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
async fn detect_wayland_display() -> LocalDisplay {
    use ionconnect_input::wayland::WaylandCaptureSession;

    let session = match WaylandCaptureSession::connect().await {
        Ok(session) => session,
        Err(err) => {
            warn!(
                %err,
                "no se pudo negociar la sesión de captura Wayland (¿se rechazó el permiso?) — este equipo no va a capturar ni reenviar entrada"
            );
            return LocalDisplay {
                geometry: FALLBACK_GEOMETRY,
                backend: CaptureBackend::Unsupported,
            };
        }
    };

    let geometry = match session.zones().await {
        Ok((zones, _zone_set)) => bounding_box(&zones).unwrap_or(FALLBACK_GEOMETRY),
        Err(err) => {
            warn!(%err, "no se pudieron consultar las zonas de captura Wayland, usando 1920x1080 por defecto");
            FALLBACK_GEOMETRY
        }
    };

    LocalDisplay {
        geometry,
        backend: CaptureBackend::Wayland(session),
    }
}

#[cfg(not(all(unix, not(target_os = "macos"))))]
pub async fn detect_local_display() -> LocalDisplay {
    LocalDisplay {
        geometry: FALLBACK_GEOMETRY,
        backend: CaptureBackend::Unsupported,
    }
}
