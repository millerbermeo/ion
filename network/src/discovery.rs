use std::collections::HashMap;
use std::net::IpAddr;

use mdns_sd::{Receiver, ServiceDaemon, ServiceEvent, ServiceInfo};

use crate::error::NetworkError;

/// Todos los equipos `IonConnect` se anuncian bajo el mismo tipo de servicio
/// mDNS; distinguirlos entre sí es trabajo del `device_id` en el TXT record,
/// no del tipo de servicio.
const SERVICE_TYPE: &str = "_ionconnect._tcp.local.";

/// Anuncio y descubrimiento de equipos `IonConnect` en la LAN vía mDNS.
/// Sin polling: `mdns-sd` corre su propio hilo y entrega eventos por canal
/// cuando algo cambia en la red.
pub struct Discovery {
    daemon: ServiceDaemon,
}

/// Datos de un peer descubierto, ya extraídos de un `ServiceEvent::ServiceResolved`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredPeer {
    pub instance_name: String,
    pub addresses: Vec<IpAddr>,
    pub port: u16,
    pub device_id: Option<String>,
}

impl Discovery {
    /// # Errors
    ///
    /// Devuelve [`NetworkError::Discovery`] si no se pudo iniciar el daemon
    /// mDNS (por ejemplo, si no hay ninguna interfaz de red disponible).
    pub fn new() -> Result<Self, NetworkError> {
        Ok(Self {
            daemon: ServiceDaemon::new()?,
        })
    }

    /// Anuncia este equipo en la LAN. `instance_name` debe ser único en la
    /// red (p. ej. el hostname); `device_id` viaja en el TXT record para que
    /// quien lo descubra pueda decidir si ya confía en este equipo antes de
    /// intentar conectar.
    ///
    /// # Errors
    ///
    /// Devuelve [`NetworkError::Discovery`] si `mdns-sd` rechaza el registro.
    pub fn advertise(
        &self,
        instance_name: &str,
        device_id: &str,
        port: u16,
    ) -> Result<(), NetworkError> {
        let host_name = format!("{instance_name}.local.");
        let mut properties = HashMap::new();
        properties.insert("device_id".to_string(), device_id.to_string());

        let service_info = ServiceInfo::new(
            SERVICE_TYPE,
            instance_name,
            &host_name,
            "",
            port,
            properties,
        )?
        .enable_addr_auto();
        self.daemon.register(service_info)?;
        Ok(())
    }

    /// Empieza a escuchar equipos `IonConnect` en la LAN. Los eventos llegan
    /// por el receiver devuelto — nada de polling.
    ///
    /// # Errors
    ///
    /// Devuelve [`NetworkError::Discovery`] si `mdns-sd` no puede iniciar la
    /// búsqueda.
    pub fn browse(&self) -> Result<Receiver<ServiceEvent>, NetworkError> {
        Ok(self.daemon.browse(SERVICE_TYPE)?)
    }
}

/// Extrae los datos relevantes de un evento de servicio resuelto; `None`
/// para cualquier otro tipo de evento (inicio/fin de búsqueda, etc.).
#[must_use]
pub fn peer_from_event(event: &ServiceEvent) -> Option<DiscoveredPeer> {
    let ServiceEvent::ServiceResolved(resolved) = event else {
        return None;
    };
    let instance_name = resolved
        .fullname
        .split('.')
        .next()
        .unwrap_or_default()
        .to_string();
    let device_id = resolved
        .txt_properties
        .get_property_val_str("device_id")
        .map(str::to_string);
    Some(DiscoveredPeer {
        instance_name,
        addresses: resolved
            .addresses
            .iter()
            .map(mdns_sd::ScopedIp::to_ip_addr)
            .collect(),
        port: resolved.port,
        device_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_non_resolved_events() {
        let event = ServiceEvent::SearchStarted(SERVICE_TYPE.to_string());
        assert!(peer_from_event(&event).is_none());
    }
}
