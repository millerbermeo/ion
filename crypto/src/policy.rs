/// Política de decisión ante un fingerprint de certificado nunca antes visto.
///
/// El llamador (GUI/`network`, en fases posteriores) decide cuándo alternar
/// entre ambos modos: por ejemplo, abrir una ventana breve en
/// `AutoTrustOnFirstUse` cuando el usuario pulsa "agregar equipo" en la GUI,
/// y volver a `RejectUnknown` el resto del tiempo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairingMode {
    /// Confía automáticamente en cualquier fingerprint no visto y lo persiste.
    /// Solo debe usarse durante una ventana de emparejamiento explícita.
    AutoTrustOnFirstUse,
    /// Rechaza cualquier fingerprint que no esté ya en el `TrustStore`.
    RejectUnknown,
}
