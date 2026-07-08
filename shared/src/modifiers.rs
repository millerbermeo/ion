use std::ops::BitOr;

/// Teclas modificadoras activas durante un evento de teclado, empaquetadas
/// en un único byte para el layout binario de `KeyboardPress`/`KeyboardRelease`.
///
/// Se envían junto al keycode (en vez de inferirse solo del lado receptor)
/// para tolerar la pérdida ocasional de algún evento de modificador sin que
/// el estado de Ctrl/Alt/Shift quede desincronizado entre equipos.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct KeyModifiers(u8);

impl KeyModifiers {
    pub const NONE: Self = Self(0);
    pub const CTRL: Self = Self(1 << 0);
    pub const ALT: Self = Self(1 << 1);
    pub const SHIFT: Self = Self(1 << 2);
    pub const SUPER: Self = Self(1 << 3);
    pub const ALT_GR: Self = Self(1 << 4);

    #[must_use]
    pub const fn from_bits(bits: u8) -> Self {
        Self(bits)
    }

    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }

    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl BitOr for KeyModifiers {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        self.union(rhs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combines_and_detects_membership() {
        let combo = KeyModifiers::CTRL | KeyModifiers::SHIFT;
        assert!(combo.contains(KeyModifiers::CTRL));
        assert!(combo.contains(KeyModifiers::SHIFT));
        assert!(!combo.contains(KeyModifiers::ALT));
    }

    #[test]
    fn round_trips_through_bits() {
        let combo = KeyModifiers::ALT_GR | KeyModifiers::SUPER;
        assert_eq!(KeyModifiers::from_bits(combo.bits()), combo);
    }
}
