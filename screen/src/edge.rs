/// Borde de un escritorio virtual por el que el cursor puede "salir" hacia
/// el equipo vecino configurado en ese borde.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScreenEdge {
    Left,
    Right,
    Top,
    Bottom,
}

impl ScreenEdge {
    /// El borde opuesto — el caso común es enlazar el borde derecho de un
    /// equipo con el borde izquierdo del vecino a su derecha.
    #[must_use]
    pub const fn opposite(self) -> Self {
        match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
            Self::Top => Self::Bottom,
            Self::Bottom => Self::Top,
        }
    }

    /// `true` para `Left`/`Right`: bordes cuya posición a lo largo se mide
    /// con `y`. `false` para `Top`/`Bottom`, medidos con `x`.
    #[must_use]
    pub const fn is_vertical(self) -> bool {
        matches!(self, Self::Left | Self::Right)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opposite_is_involutive() {
        for edge in [
            ScreenEdge::Left,
            ScreenEdge::Right,
            ScreenEdge::Top,
            ScreenEdge::Bottom,
        ] {
            assert_eq!(edge.opposite().opposite(), edge);
        }
    }

    #[test]
    fn left_and_right_are_vertical() {
        assert!(ScreenEdge::Left.is_vertical());
        assert!(ScreenEdge::Right.is_vertical());
        assert!(!ScreenEdge::Top.is_vertical());
        assert!(!ScreenEdge::Bottom.is_vertical());
    }
}
