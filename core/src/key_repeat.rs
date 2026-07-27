//! Repetición automática de teclas mantenidas.
//!
//! Hace falta sintetizarla porque los backends de captura no la entregan:
//! los eventos `XI_RawKeyPress` de X11 reportan la pulsación física una sola
//! vez y nunca las repeticiones que el servidor X genera después (medido
//! contra un servidor real: 1 evento crudo frente a 22 cocidos en 1,5 s),
//! y el stream EIS del portal de Wayland se comporta igual. Sin esto, del
//! otro lado del enlace mantener Backspace borra un solo carácter y
//! mantener una flecha mueve el cursor una sola posición.
//!
//! Este módulo es lógica pura — no sabe de red, ni de X11, ni de relojes
//! del sistema: recibe pulsaciones/liberaciones y un `Instant`, y responde
//! cuándo toca emitir la próxima repetición. Quien orqueste la sesión de
//! captura (ver `crate::input_session`) es el que despierta a tiempo y
//! manda el `KeyboardPress` correspondiente.

use std::time::{Duration, Instant};

/// Estado de la tecla que está repitiéndose en este momento.
struct Repeating {
    keycode: u32,
    /// Cuándo corresponde emitir la próxima repetición.
    next: Instant,
}

/// Genera repeticiones de la tecla mantenida, replicando el comportamiento
/// de un teclado local: solo repite la **última** tecla presionada (igual
/// que el hardware real: apretar una segunda tecla le roba la repetición a
/// la primera), respeta el retardo inicial antes de arrancar, y no repite
/// las teclas que el sistema excluye (modificadores).
pub struct KeyRepeater {
    delay: Duration,
    interval: Duration,
    /// Qué teclas pueden repetirse. Se consulta una vez por pulsación.
    repeatable: Box<dyn Fn(u32) -> bool + Send>,
    current: Option<Repeating>,
}

impl KeyRepeater {
    /// `repeatable` decide qué keycodes participan de la repetición —
    /// típicamente el bitmask real del servidor X
    /// (`ionconnect_input::x11::KeyRepeatSettings::repeats`), que ya excluye
    /// los modificadores.
    pub fn new(
        delay: Duration,
        interval: Duration,
        repeatable: impl Fn(u32) -> bool + Send + 'static,
    ) -> Self {
        Self {
            delay,
            interval,
            repeatable: Box::new(repeatable),
            current: None,
        }
    }

    /// Registra que se presionó `keycode`. Pasa a ser la tecla que repite
    /// (desplazando a la anterior, si había), salvo que el sistema la
    /// excluya — en cuyo caso corta cualquier repetición en curso, igual que
    /// hace un teclado real cuando se aprieta un modificador.
    pub fn on_press(&mut self, keycode: u32, now: Instant) {
        if !(self.repeatable)(keycode) {
            self.current = None;
            return;
        }
        self.current = Some(Repeating {
            keycode,
            next: now + self.delay,
        });
    }

    /// Registra que se soltó `keycode`. Solo detiene la repetición si es
    /// justamente la tecla que estaba repitiéndose: soltar otra cualquiera
    /// no debería cortarla.
    pub fn on_release(&mut self, keycode: u32) {
        if self.current.as_ref().is_some_and(|r| r.keycode == keycode) {
            self.current = None;
        }
    }

    /// Corta toda repetición en curso — llamar cuando el control deja de
    /// estar cedido a un equipo remoto (hand-off de vuelta, peer
    /// desconectado). Sin esto, una tecla que quedó "mantenida" en el
    /// momento del cambio seguiría repitiéndose contra un destino que ya no
    /// corresponde.
    pub fn clear(&mut self) {
        self.current = None;
    }

    /// Cuándo hay que volver a llamar a [`Self::tick`]. `None` = no hay
    /// ninguna tecla repitiéndose, así que se puede esperar indefinidamente
    /// al próximo evento real.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Instant> {
        self.current.as_ref().map(|r| r.next)
    }

    /// Keycode a repetir si ya venció el plazo, avanzando el estado interno
    /// para la próxima. Devuelve `None` si todavía no es momento o si no hay
    /// nada mantenido.
    ///
    /// Deliberadamente emite **una sola** repetición por llamada aunque haya
    /// pasado mucho tiempo: si el proceso se quedó sin CPU un rato (o la
    /// máquina volvió de suspensión), acumular la deuda y descargarla de
    /// golpe metería una ráfaga de decenas de pulsaciones que el usuario no
    /// pidió. Por eso el próximo plazo se calcula desde `now` y no desde el
    /// vencido.
    pub fn tick(&mut self, now: Instant) -> Option<u32> {
        let repeating = self.current.as_mut()?;
        if now < repeating.next {
            return None;
        }
        repeating.next = now + self.interval;
        Some(repeating.keycode)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repeater(now: Instant) -> (KeyRepeater, Instant) {
        // Solo la tecla 99 está excluida, para poder probar el filtro.
        let repeater = KeyRepeater::new(
            Duration::from_millis(500),
            Duration::from_millis(30),
            |keycode| keycode != 99,
        );
        (repeater, now)
    }

    #[test]
    fn does_not_repeat_before_the_initial_delay() {
        let now = Instant::now();
        let (mut repeater, now) = repeater(now);
        repeater.on_press(30, now);

        assert_eq!(repeater.tick(now), None);
        assert_eq!(repeater.tick(now + Duration::from_millis(499)), None);
    }

    #[test]
    fn repeats_at_the_configured_interval_after_the_delay() {
        let now = Instant::now();
        let (mut repeater, now) = repeater(now);
        repeater.on_press(30, now);

        let first = now + Duration::from_millis(500);
        assert_eq!(repeater.tick(first), Some(30));
        // Inmediatamente después no toca todavía.
        assert_eq!(repeater.tick(first), None);
        assert_eq!(repeater.tick(first + Duration::from_millis(30)), Some(30));
        assert_eq!(repeater.tick(first + Duration::from_millis(60)), Some(30));
    }

    #[test]
    fn releasing_the_key_stops_the_repetition() {
        let now = Instant::now();
        let (mut repeater, now) = repeater(now);
        repeater.on_press(30, now);
        repeater.on_release(30);

        assert_eq!(repeater.tick(now + Duration::from_secs(5)), None);
        assert_eq!(repeater.next_deadline(), None);
    }

    #[test]
    fn releasing_a_different_key_does_not_stop_the_repetition() {
        let now = Instant::now();
        let (mut repeater, now) = repeater(now);
        repeater.on_press(30, now);
        repeater.on_release(31);

        assert_eq!(repeater.tick(now + Duration::from_millis(500)), Some(30));
    }

    #[test]
    fn the_last_key_pressed_takes_over_the_repetition() {
        let now = Instant::now();
        let (mut repeater, now) = repeater(now);
        repeater.on_press(30, now);
        // A mitad del retardo inicial se aprieta otra: como en un teclado
        // real, pasa a repetir la nueva y desde cero.
        let second = now + Duration::from_millis(250);
        repeater.on_press(31, second);

        assert_eq!(repeater.tick(now + Duration::from_millis(500)), None);
        assert_eq!(repeater.tick(second + Duration::from_millis(500)), Some(31));
    }

    #[test]
    fn a_non_repeatable_key_cancels_instead_of_repeating() {
        let now = Instant::now();
        let (mut repeater, now) = repeater(now);
        repeater.on_press(30, now);
        repeater.on_press(99, now + Duration::from_millis(10));

        assert_eq!(repeater.tick(now + Duration::from_secs(1)), None);
        assert_eq!(repeater.next_deadline(), None);
    }

    #[test]
    fn clear_stops_everything() {
        let now = Instant::now();
        let (mut repeater, now) = repeater(now);
        repeater.on_press(30, now);
        repeater.clear();

        assert_eq!(repeater.tick(now + Duration::from_secs(1)), None);
    }

    #[test]
    fn next_deadline_tracks_the_pending_repetition() {
        let now = Instant::now();
        let (mut repeater, now) = repeater(now);
        assert_eq!(repeater.next_deadline(), None);

        repeater.on_press(30, now);
        assert_eq!(
            repeater.next_deadline(),
            Some(now + Duration::from_millis(500))
        );

        let fired = now + Duration::from_millis(500);
        repeater.tick(fired);
        assert_eq!(
            repeater.next_deadline(),
            Some(fired + Duration::from_millis(30))
        );
    }

    #[test]
    fn a_long_stall_does_not_produce_a_burst_of_catch_up_repetitions() {
        let now = Instant::now();
        let (mut repeater, now) = repeater(now);
        repeater.on_press(30, now);

        // El proceso se quedó sin CPU 5 segundos: una sola repetición, no
        // las ~150 que "correspondían".
        let late = now + Duration::from_secs(5);
        assert_eq!(repeater.tick(late), Some(30));
        assert_eq!(repeater.tick(late), None);
    }
}
