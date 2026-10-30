//! The pages of a chat a client scrolls back through, chosen under the chat's lock and read
//! from disk once it is let go.

use sikemux_fold::{ChatEvent, ChatEventKind, PageOf};

use super::super::super::history::{History, Page, Reader};
use super::super::PAGE_BYTES;
use super::Replay;

/// Which rows of a chat a client asks for, by event index.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Anchor {
    Before(u64),
    After(u64),
    /// The row holding the event, and about half a page either side of it.
    Around(u64),
}

/// The events a page folds, chosen under the chat's lock. The part on disk is read once the
/// lock is let go; the newest, not yet on disk, are copied with the choice.
pub(in super::super) struct Planned {
    pub(in super::super) reader: Option<Reader>,
    pub(in super::super) lo: u64,
    pub(in super::super) hi: u64,
    pub(in super::super) held: Vec<(u64, ChatEvent)>,
    /// The chat holds more on the side the page reads away from.
    pub(in super::super) beyond: bool,
}

impl Planned {
    /// Where a client goes on from past these events, when the chat goes on past them.
    pub(in super::super) fn edge(&self, side: PageOf) -> Option<u64> {
        let edge = match side {
            PageOf::Before(_) => self.lo,
            PageOf::After(_) | PageOf::From(_) => self.hi.saturating_sub(1),
        };
        self.beyond.then_some(edge)
    }
}

impl Replay {
    /// The events of the turns before `before`, from the history on disk, or
    /// from the replay when the chat keeps none.
    #[cfg(test)]
    pub(crate) fn page(&self, before: u64, turns: usize) -> Result<Page, String> {
        let planned = self.plan(
            PageOf::Before(before),
            turns.clamp(1, super::super::MAX_PAGE_TURNS),
        )?;
        let mut events = match &planned.reader {
            Some(reader) => reader
                .range(planned.lo, planned.hi)
                .map_err(|error| error.to_string())?,
            None => Vec::new(),
        };
        events.extend(planned.held);
        Ok(self.with_spawns(Page {
            events,
            older_before: planned.beyond.then_some(planned.lo),
            first: planned.lo,
        }))
    }

    /// The events of the turns `side` asks for, as many as `turns` while their events stay
    /// under the page's size: whole turns, or from the nearest point inside a turn too big for
    /// a page of its own.
    pub(in super::super) fn plan(&self, side: PageOf, turns: usize) -> Result<Planned, String> {
        let reader = self
            .history
            .as_ref()
            .filter(|history| history.first().is_some())
            .map(History::reader)
            .transpose()
            .map_err(|error| error.to_string())?
            .flatten();
        let (first, on_disk) = reader.as_ref().map_or_else(
            || {
                let first = self
                    .entries
                    .front()
                    .map_or(self.next_index, |entry| entry.index);
                (first, first)
            },
            Reader::span,
        );
        let mut points: Vec<(u64, u64, bool)> = reader.iter().flat_map(Reader::points).collect();
        let mut end = reader.as_ref().map_or(0, Reader::bytes);
        for entry in self.entries.iter().filter(|entry| entry.index >= on_disk) {
            if entry.point || entry.starts_turn {
                points.push((entry.index, end, entry.starts_turn));
            }
            end += entry.bytes as u64;
        }
        let next = self.next_index;
        let budget = PAGE_BYTES;
        let forward = |from: u64| {
            let start = points
                .iter()
                .rev()
                .find(|(index, _, starts)| *index <= from && *starts)
                .or_else(|| points.iter().rev().find(|(index, _, _)| *index <= from))
                .map_or((first, 0), |&(index, at, _)| (index, at));
            let mut hi = next;
            let mut counted = 1;
            for &(index, at, starts) in points.iter().filter(|(index, _, _)| *index > start.0) {
                if at - start.1 > budget || (starts && counted >= turns) {
                    hi = index;
                    break;
                }
                counted += usize::from(starts);
            }
            (start.0, hi, hi < next)
        };
        let (lo, hi, beyond) = match side {
            PageOf::Before(before) => {
                let before = before.min(next);
                let after = |turn: bool| {
                    points
                        .iter()
                        .find(|(index, _, starts)| *index >= before && (*starts || !turn))
                        .map(|&(index, at, _)| (index, at))
                };
                let (hi, to) = match (after(true), after(false)) {
                    (Some(turn), Some(point)) if turn.1 - point.1 <= budget => turn,
                    (_, Some(point)) => point,
                    _ => (next, end),
                };
                let earlier: Vec<&(u64, u64, bool)> = points
                    .iter()
                    .rev()
                    .filter(|(index, _, _)| *index < before)
                    .collect();
                let (mut turn, mut piece, mut counted) = (None, None, 0);
                for &&(index, at, starts) in &earlier {
                    if to - at > budget {
                        break;
                    }
                    piece = Some(index);
                    if starts {
                        turn = Some(index);
                        counted += 1;
                        if counted >= turns {
                            break;
                        }
                    }
                }
                let lo = turn
                    .or(piece)
                    .or_else(|| earlier.first().map(|point| point.0))
                    .unwrap_or(first);
                (lo, hi, lo > first)
            }
            PageOf::After(after) => forward(after.saturating_add(1)),
            PageOf::From(from) => forward(from),
        };
        let held = self
            .entries
            .iter()
            .filter(|entry| entry.index >= on_disk && (lo..hi).contains(&entry.index))
            .filter(|entry| entry.event.kind != ChatEventKind::PermissionRequest)
            .map(|entry| (entry.index, entry.event.clone()))
            .collect();
        Ok(Planned {
            reader,
            lo,
            hi,
            held,
            beyond,
        })
    }

    /// `page` with the starts of the subagents it speaks of that began before it.
    pub(in super::super) fn with_spawns(&self, mut page: Page) -> Page {
        let spawns: Vec<(u64, ChatEvent)> = self
            .spawns_before(page.events.iter().map(|(_, event)| event), page.first)
            .into_iter()
            .map(|(index, event)| (index, event.clone()))
            .collect();
        page.events.splice(0..0, spawns);
        page
    }
}
