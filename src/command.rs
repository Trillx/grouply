//! Requests a tab bar sends to the controller, packed into a message parameter.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabCommand {
    Activate(usize),
    Close(usize),
    /// Pull the tab out of its group, dropping the window at the cursor.
    Detach(usize),
    Reorder {
        from: usize,
        to: usize,
    },
    Ungroup,
    CloseAll,
}

const INDEX_MASK: u64 = 0xFF_FFFF;

impl TabCommand {
    /// Layout: kind in bits 0..8, first index in bits 8..32, second index in bits 32..56.
    pub fn encode(self) -> u64 {
        let (kind, a, b) = match self {
            Self::Activate(i) => (1, i, 0),
            Self::Close(i) => (2, i, 0),
            Self::Detach(i) => (3, i, 0),
            Self::Reorder { from, to } => (4, from, to),
            Self::Ungroup => (5, 0, 0),
            Self::CloseAll => (6, 0, 0),
        };
        kind | (a as u64 & INDEX_MASK) << 8 | (b as u64 & INDEX_MASK) << 32
    }

    pub fn decode(v: u64) -> Option<Self> {
        let a = ((v >> 8) & INDEX_MASK) as usize;
        let b = ((v >> 32) & INDEX_MASK) as usize;
        Some(match v & 0xFF {
            1 => Self::Activate(a),
            2 => Self::Close(a),
            3 => Self::Detach(a),
            4 => Self::Reorder { from: a, to: b },
            5 => Self::Ungroup,
            6 => Self::CloseAll,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        for cmd in [
            TabCommand::Activate(0),
            TabCommand::Close(7),
            TabCommand::Detach(123),
            TabCommand::Reorder { from: 3, to: 0 },
            TabCommand::Reorder {
                from: 0,
                to: 0xFF_FFFF,
            },
            TabCommand::Ungroup,
            TabCommand::CloseAll,
        ] {
            assert_eq!(TabCommand::decode(cmd.encode()), Some(cmd));
        }
    }

    #[test]
    fn rejects_unknown_kinds() {
        assert_eq!(TabCommand::decode(0), None);
        assert_eq!(TabCommand::decode(0xFF), None);
    }
}
