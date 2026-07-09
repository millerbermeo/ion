use ionconnect_protocol::MouseButton;
use x11rb::protocol::xinput::Fp3232;

/// X11 core protocol no tiene botones "back"/"forward" estándar; se mapean
/// a los códigos 8/9 que la inmensa mayoría de mice y drivers usan. El
/// scroll wheel tampoco es un eje separado en el protocolo core/XTEST: cada
/// muesca es un click+release del botón 4 (arriba), 5 (abajo), 6
/// (izquierda) o 7 (derecha) — convención estándar de X11 desde XFree86.
pub(super) const fn button_to_code(button: MouseButton) -> u8 {
    match button {
        MouseButton::Left => 1,
        MouseButton::Middle => 2,
        MouseButton::Right => 3,
        MouseButton::ScrollUp => 4,
        MouseButton::ScrollDown => 5,
        MouseButton::ScrollLeft => 6,
        MouseButton::ScrollRight => 7,
        MouseButton::Back => 8,
        MouseButton::Forward => 9,
    }
}

pub(super) const fn button_from_code(code: u32) -> Option<MouseButton> {
    match code {
        1 => Some(MouseButton::Left),
        2 => Some(MouseButton::Middle),
        3 => Some(MouseButton::Right),
        4 => Some(MouseButton::ScrollUp),
        5 => Some(MouseButton::ScrollDown),
        6 => Some(MouseButton::ScrollLeft),
        7 => Some(MouseButton::ScrollRight),
        8 => Some(MouseButton::Back),
        9 => Some(MouseButton::Forward),
        _ => None,
    }
}

/// Extrae el valor del valuador `index` de un evento XI2 crudo (`RawMotion`,
/// `RawButtonPress`, etc.), si está presente en `mask`.
///
/// Los eventos XI2 solo incluyen en `values` los valuadores cuyo bit está
/// activo en `mask`, en orden ascendente — hay que contar bits activos, no
/// indexar directamente.
pub(super) fn valuator_value(mask: &[u32], values: &[Fp3232], index: u16) -> Option<f64> {
    let index = usize::from(index);
    let mut cursor = 0usize;
    for bit in 0..(mask.len() * 32) {
        let word = bit / 32;
        let offset = bit % 32;
        if mask[word] & (1 << offset) == 0 {
            continue;
        }
        if bit == index {
            let value = values.get(cursor)?;
            return Some(f64::from(value.integral) + f64::from(value.frac) / f64::from(u32::MAX));
        }
        cursor += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_button_codes() {
        for button in [
            MouseButton::Left,
            MouseButton::Middle,
            MouseButton::Right,
            MouseButton::Back,
            MouseButton::Forward,
            MouseButton::ScrollUp,
            MouseButton::ScrollDown,
            MouseButton::ScrollLeft,
            MouseButton::ScrollRight,
        ] {
            assert_eq!(
                button_from_code(u32::from(button_to_code(button))),
                Some(button)
            );
        }
    }

    #[test]
    fn extracts_second_present_valuator() {
        // Bits 0 y 2 activos (valuadores 0 y 2); el valuador 1 está ausente.
        let mask = [0b0000_0101];
        let values = [
            Fp3232 {
                integral: 3,
                frac: 0,
            },
            Fp3232 {
                integral: -7,
                frac: 0,
            },
        ];
        assert_eq!(valuator_value(&mask, &values, 0), Some(3.0));
        assert_eq!(valuator_value(&mask, &values, 1), None);
        assert_eq!(valuator_value(&mask, &values, 2), Some(-7.0));
    }
}
