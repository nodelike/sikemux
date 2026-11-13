//! What the core keeps of the boards, and how the rest reads it.

use super::{board, Core};

pub(crate) struct BoardFields {
    pub(crate) board_above: super::board_above::BoardAbove,
    pub(crate) boards: board::Boards,
}

pub(crate) trait BoardParts {
    fn board_above(&self) -> &super::board_above::BoardAbove;
    fn boards(&self) -> &board::Boards;
}

impl BoardParts for Core {
    fn board_above(&self) -> &super::board_above::BoardAbove {
        &self.kept::<BoardFields>().board_above
    }

    fn boards(&self) -> &board::Boards {
        &self.kept::<BoardFields>().boards
    }
}
