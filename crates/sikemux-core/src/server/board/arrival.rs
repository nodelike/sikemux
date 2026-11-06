//! What identifies work arriving on the board, so the same work never
//! starts twice: a ticket by its key, a trigger by its id, CI failing on the
//! project's branch by its checks and tests. The person's or an agent's own
//! words carry none. Work that arrives with a key an open card
//! already holds is kept on that card instead, the newest few times, and
//! whoever answers for the card hears.

use serde::{Deserialize, Serialize};
use sikemux_process::clock::epoch_ms;

use super::failing::Broke;
use super::{Adding, Board, Card, Notice, Status};
use crate::text::clipped;

/// Arrivals a card keeps; the oldest go first.
const MAX_ARRIVALS: usize = 8;
const MAX_DETAILS_CHARS: usize = 4000;

/// Work that arrived again for a card that already held it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Arrival {
    /// The ticket's, trigger's or CI failure's title.
    pub title: String,
    /// What came with it, such as a webhook's request or what CI printed, cut
    /// to what a card keeps.
    pub details: String,
    pub at: u64,
}

pub(super) enum Key<'a> {
    Ticket(&'a str),
    Trigger(u64),
    /// Any of what CI failed on.
    Ci(&'a Broke),
}

impl<'a> Key<'a> {
    pub(super) fn of(adding: &'a Adding) -> Option<Self> {
        match (&adding.ticket, adding.trigger, &adding.ci) {
            (Some(ticket), _, _) => Some(Key::Ticket(&ticket.key)),
            (None, Some(trigger), _) => Some(Key::Trigger(trigger)),
            (None, None, Some(broke)) => Some(Key::Ci(broke)),
            (None, None, None) => None,
        }
    }

    fn held_by(&self, card: &Card) -> bool {
        match self {
            Key::Ticket(key) => card
                .ticket
                .as_ref()
                .is_some_and(|ticket| ticket.key.eq_ignore_ascii_case(key)),
            Key::Trigger(trigger) => card.trigger == Some(*trigger),
            Key::Ci(broke) => broke.held_by(&card.ci),
        }
    }
}

pub(super) fn holder(board: &Board, key: &Key) -> Option<u64> {
    board
        .cards
        .iter()
        .find(|card| card.status != Status::Done && key.held_by(card))
        .map(|card| card.id)
}

/// Keeps `adding` on `card`, which held its key already, and answers what the
/// agent answering for the card is told: its owner, else the lead of the
/// team it was last in.
pub(super) fn arrived_again(card: &mut Card, adding: &Adding) -> Option<Notice> {
    if card.arrivals.len() >= MAX_ARRIVALS {
        card.arrivals.remove(0);
    }
    let at = epoch_ms();
    let details = adding
        .ci
        .as_ref()
        .map_or(&adding.details, |broke| &broke.output);
    card.arrivals.push(Arrival {
        title: adding.title.clone(),
        details: clipped(details, MAX_DETAILS_CHARS),
        at,
    });
    if let Some(broke) = &adding.ci {
        broke.hold(&mut card.ci);
    }
    card.updated_at = at;
    let to = card
        .owner_id()
        .map(str::to_owned)
        .or_else(|| card.team.as_ref().map(|team| team.lead.clone()))?;
    let what = match Key::of(adding)? {
        Key::Ticket(key) => format!("Ticket {key} was put on the board again"),
        Key::Trigger(_) => format!("The trigger \"{}\" fired again", adding.title),
        Key::Ci(broke) => broke.said(),
    };
    Some(Notice {
        to,
        text: format!(
            "{what}, and it went to {}, which already holds it, rather than a new card. board_list with card {} shows what arrived, under arrivals.",
            card.label(),
            card.id
        ),
    })
}
