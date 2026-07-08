/// Evita el bucle infinito clásico de sincronización de portapapeles: A
/// recibe un `ClipboardSync` de B y escribe en su propio portapapeles →
/// el watcher de A lo detecta como "cambio" → A se lo reenvía a B → B lo
/// vuelve a detectar como cambio → ida y vuelta indefinida.
///
/// Estrategia: recordar el último contenido que *nosotros* pusimos, y no
/// tratarlo como una novedad genuina cuando el próximo poll lo vuelva a
/// leer. Puramente por valor — no le importa quién originó cada cambio,
/// solo si el contenido observado ya es conocido.
#[derive(Debug, Default)]
pub struct LoopGuard {
    last_known: Option<String>,
}

impl LoopGuard {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Llamar justo después de escribir `text` en el portapapeles local a
    /// pedido de un peer remoto, para no reenviarlo de vuelta.
    pub fn record_own_write(&mut self, text: String) {
        self.last_known = Some(text);
    }

    /// Llamar con cada valor leído del portapapeles local. Devuelve
    /// `Some(text)` si representa un cambio genuino que hay que reenviar a
    /// los peers, o `None` si es idéntico al último contenido conocido
    /// (incluyendo el eco de nuestra propia escritura).
    pub fn observe(&mut self, text: String) -> Option<String> {
        if self.last_known.as_deref() == Some(text.as_str()) {
            return None;
        }
        self.last_known = Some(text.clone());
        Some(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_observation_is_always_a_change() {
        let mut guard = LoopGuard::new();
        assert_eq!(guard.observe("hola".to_string()), Some("hola".to_string()));
    }

    #[test]
    fn repeating_the_same_value_is_not_a_change() {
        let mut guard = LoopGuard::new();
        guard.observe("hola".to_string());
        assert_eq!(guard.observe("hola".to_string()), None);
    }

    #[test]
    fn own_write_is_not_echoed_back_as_a_change() {
        let mut guard = LoopGuard::new();
        guard.record_own_write("desde el peer".to_string());
        // El watcher local hace poll y lee justo lo que acabamos de escribir.
        assert_eq!(guard.observe("desde el peer".to_string()), None);
    }

    #[test]
    fn a_genuinely_new_value_after_own_write_is_still_detected() {
        let mut guard = LoopGuard::new();
        guard.record_own_write("desde el peer".to_string());
        guard.observe("desde el peer".to_string());
        // El usuario copia algo distinto a mano después.
        assert_eq!(
            guard.observe("copiado a mano".to_string()),
            Some("copiado a mano".to_string())
        );
    }
}
