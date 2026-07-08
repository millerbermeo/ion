use std::fmt;
use std::future::Future;
use std::time::Duration;

use tracing::warn;

/// Parámetros de reintento exponencial. No abrir conexiones constantemente:
/// cada fallo espera más que el anterior, hasta un techo.
#[derive(Debug, Clone, Copy)]
pub struct BackoffPolicy {
    pub initial: Duration,
    pub max: Duration,
    pub multiplier: f64,
}

impl Default for BackoffPolicy {
    fn default() -> Self {
        Self {
            initial: Duration::from_millis(500),
            max: Duration::from_secs(30),
            multiplier: 2.0,
        }
    }
}

/// Estado mutable de un backoff en curso. Se reinicia a `initial` cuando la
/// conexión se restablece con éxito.
#[derive(Debug, Clone, Copy)]
pub struct Backoff {
    policy: BackoffPolicy,
    next: Duration,
}

impl Backoff {
    #[must_use]
    pub fn new(policy: BackoffPolicy) -> Self {
        Self {
            next: policy.initial,
            policy,
        }
    }

    /// Duración a esperar antes del próximo intento; avanza el estado
    /// interno para el siguiente fallo.
    pub fn next_delay(&mut self) -> Duration {
        let delay = self.next;
        let scaled = self.next.mul_f64(self.policy.multiplier);
        self.next = scaled.min(self.policy.max);
        delay
    }

    pub fn reset(&mut self) {
        self.next = self.policy.initial;
    }
}

/// Reintenta `connect` indefinidamente con backoff exponencial hasta que
/// tenga éxito. Pensado para una conexión persistente que siempre debe
/// reestablecerse (el llamador decide cuándo abandonar, p. ej. cancelando la
/// tarea de tokio que la ejecuta).
pub async fn connect_with_backoff<F, Fut, T, E>(policy: BackoffPolicy, mut connect: F) -> T
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    E: fmt::Display,
{
    let mut backoff = Backoff::new(policy);
    loop {
        match connect().await {
            Ok(value) => return value,
            Err(error) => {
                let delay = backoff.next_delay();
                warn!(%error, delay_ms = delay.as_millis(), "reintentando conexión");
                tokio::time::sleep(delay).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    #[test]
    fn delays_grow_exponentially_and_cap_at_max() {
        let policy = BackoffPolicy {
            initial: Duration::from_millis(100),
            max: Duration::from_millis(500),
            multiplier: 2.0,
        };
        let mut backoff = Backoff::new(policy);
        assert_eq!(backoff.next_delay(), Duration::from_millis(100));
        assert_eq!(backoff.next_delay(), Duration::from_millis(200));
        assert_eq!(backoff.next_delay(), Duration::from_millis(400));
        // 800ms superaría el techo de 500ms.
        assert_eq!(backoff.next_delay(), Duration::from_millis(500));
        assert_eq!(backoff.next_delay(), Duration::from_millis(500));
    }

    #[test]
    fn reset_returns_to_initial_delay() {
        let policy = BackoffPolicy {
            initial: Duration::from_millis(100),
            max: Duration::from_millis(500),
            multiplier: 2.0,
        };
        let mut backoff = Backoff::new(policy);
        backoff.next_delay();
        backoff.next_delay();
        backoff.reset();
        assert_eq!(backoff.next_delay(), Duration::from_millis(100));
    }

    #[tokio::test(start_paused = true)]
    async fn connect_with_backoff_retries_until_success() {
        let attempts = AtomicU32::new(0);
        let policy = BackoffPolicy {
            initial: Duration::from_millis(10),
            max: Duration::from_millis(40),
            multiplier: 2.0,
        };

        let result = connect_with_backoff(policy, || {
            let attempt = attempts.fetch_add(1, Ordering::SeqCst);
            async move {
                if attempt < 2 {
                    Err("todavía no")
                } else {
                    Ok(attempt)
                }
            }
        })
        .await;

        assert_eq!(result, 2);
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }
}
