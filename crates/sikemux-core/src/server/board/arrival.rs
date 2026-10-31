//! What identifies work arriving on the board, so the same work never
//! starts twice: a ticket by its key, a trigger by its id. The person's or an
//! agent's own words carry none. Work that arrives with a key an open card
//! already holds goes to that card instead, and whoever answers for it hears.

use super::{Adding, Board, Card, Notice, Status};
use crate::text::clipped;

/// How much of what arrived again its card's owner is told.
const TOLD_CHARS: usize = 600;

pub(super) enum Key<'a> {
    Ticket(&'a str),
    Trigger(u64),
}

impl<'a> Key<'a> {
    pub(super) fn of(adding: &'a Adding) -> Option<Self> {
        match (&adding.ticket, adding.trigger) {
            (Some(ticket), _) => Some(Key::Ticket(&ticket.key)),
            (None, Some(trigger)) => Some(Key::Trigger(trigger)),
            (None, None) => None,
        }
    }

    fn held_by(&self, card: &Card) -> bool {
        match self {
            Key::Ticket(key) => card
                .ticket
                .as_ref()
                .is_some_and(|ticket| ticket.key.eq_ignore_ascii_case(key)),
            Key::Trigger(trigger) => card.trigger == Some(*trigger),
        }
    }
}

/// The open card that already holds `key`.
pub(super) fn holder(board: &Board, key: &Key) -> Option<u64> {
    board
        .cards
        .iter()
        .find(|card| card.status != Status::Done && key.held_by(card))
        .map(|card| card.id)
}

/// What the agent answering for `card` is told when `adding` arrived again
/// and went to it: its owner, else the lead of the team it was last in.
pub(super) fn arrived_again(card: &Card, adding: &Adding) -> Option<Notice> {
    let to = card
        .owner_id()
        .map(str::to_owned)
        .or_else(|| card.team.as_ref().map(|team| team.lead.clone()))?;
    let what = match Key::of(adding)? {
        Key::Ticket(key) => format!("Ticket {key} was put on the board again"),
        Key::Trigger(_) => format!("The trigger \"{}\" fired again", adding.title),
    };
    let details = clipped(&adding.details, TOLD_CHARS);
    Some(Notice {
        to,
        text: format!(
            "{what}, and it went to {}, which already holds it, rather than a new card.{}",
            card.label(),
            if details.is_empty() {
                String::new()
            } else {
                format!("\n\n{details}")
            }
        ),
    })
}
