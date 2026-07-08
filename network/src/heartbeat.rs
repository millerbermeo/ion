use tokio::time::{Duration, Instant};

/// Rastrea si una conexión sigue viva a partir del último `Heartbeat`
/// recibido. Usa `tokio::time::Instant` (no `std::time::Instant`) para poder
/// probar la lógica con instantes construidos a mano, sin depender de
/// tiempo real ni de `tokio::time::pause`.
#[derive(Debug, Clone, Copy)]
pub struct HeartbeatMonitor {
    last_seen: Instant,
    timeout: Duration,
}

impl HeartbeatMonitor {
    #[must_use]
    pub fn new(timeout: Duration) -> Self {
        Self {
            last_seen: Instant::now(),
            timeout,
        }
    }

    pub fn record_received_at(&mut self, now: Instant) {
        self.last_seen = now;
    }

    pub fn record_received(&mut self) {
        self.record_received_at(Instant::now());
    }

    #[must_use]
    pub fn is_alive_at(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.last_seen) < self.timeout
    }

    #[must_use]
    pub fn is_alive(&self) -> bool {
        self.is_alive_at(Instant::now())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stays_alive_before_timeout_elapses() {
        let start = Instant::now();
        let monitor = HeartbeatMonitor {
            last_seen: start,
            timeout: Duration::from_secs(10),
        };
        assert!(monitor.is_alive_at(start + Duration::from_secs(9)));
    }

    #[test]
    fn dies_after_timeout_elapses() {
        let start = Instant::now();
        let monitor = HeartbeatMonitor {
            last_seen: start,
            timeout: Duration::from_secs(10),
        };
        assert!(!monitor.is_alive_at(start + Duration::from_secs(11)));
    }

    #[test]
    fn recording_a_heartbeat_resets_the_deadline() {
        let start = Instant::now();
        let mut monitor = HeartbeatMonitor {
            last_seen: start,
            timeout: Duration::from_secs(10),
        };
        let almost_dead = start + Duration::from_secs(9);
        monitor.record_received_at(almost_dead);
        assert!(monitor.is_alive_at(almost_dead + Duration::from_secs(9)));
    }
}
