//! What the core keeps of the boards, and how the rest reads it.

use super::{board, Core};

pub struct BoardFields {
    pub board_above: super::board_above::BoardAbove,
    pub boards: board::Boards,
}

pub trait BoardParts {
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
