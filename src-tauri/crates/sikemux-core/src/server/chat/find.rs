//! Finding words anywhere in a chat, in the rows held in memory and back through its history
//! on disk, a bounded step at a time: each call answers what it found and where to go on from.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sikemux_fold::{matches, searchable, with_trees, PageOf, Query};
use sikemux_wire::chat::{ChatMessage, ChatPart};

/// The most hits one call answers, and how many unless asked for fewer.
const MOST_HITS: usize = 200;
const HITS: usize = 50;
/// What one call reads at most, of rows and of time, before it answers and says where to go on.
const STEP_BYTES: usize = 4 * 1024 * 1024;
const STEP_TIME: Duration = Duration::from_millis(250);
/// A batch of rows read at a time.
const BATCH_BYTES: usize = 512 * 1024;
/// The words shown either side of a hit.
const PREVIEW_CHARS: usize = 30;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Direction {
    #[default]
    Older,
    Newer,
}

/// Where a find goes on from: a place in a row's text, and how many it had counted to there.
/// An empty `part` stands before or after every part of its row, as the direction goes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FindAt {
    pub event: u64,
    #[serde(default)]
    pub part: String,
    #[serde(default)]
    pub offset: usize,
    #[serde(default)]
    pub counted: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FindParams {
    pub agent_id: String,
    pub feed: String,
    pub query: String,
    #[serde(default)]
    pub match_case: bool,
    #[serde(default)]
    pub whole_word: bool,
    pub from: Option<FindAt>,
    #[serde(default)]
    pub direction: Direction,
    pub limit: Option<usize>,
    #[serde(default)]
    pub count_only: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Hit {
    pub row: String,
    pub from_event: u64,
    pub part: String,
    /// UTF-16 offsets in the part's searchable text.
    pub range: [usize; 2],
    pub preview: String,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Scanned {
    pub from_event: Option<u64>,
    pub to_event: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Counted {
    pub matches: u64,
    pub complete: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FindAnswer {
    pub hits: Vec<Hit>,
    /// Where to go on from; none once the chat's start, or end, is reached.
    pub next: Option<FindAt>,
    pub scanned: Scanned,
    pub counted: Counted,
}

/// What a find reads its rows from: the next batch past an event, as clients are sent them.
pub(crate) trait Rows {
    /// The rows `wanted` asks for nearest the event first, about `budget` bytes of them, and
    /// whether none are left beyond them.
    fn batch(&self, wanted: PageOf, budget: usize) -> Result<(Vec<ChatMessage>, bool), String>;
    /// Whether a newer find from the same client took this one's place.
    fn superseded(&self) -> bool;
}

/// One step of a find through `rows`.
pub(crate) fn step(rows: &impl Rows, params: &FindParams) -> Result<FindAnswer, String> {
    let query = Query {
        text: params.query.clone(),
        match_case: params.match_case,
        whole_word: params.whole_word,
    };
    let older = params.direction == Direction::Older;
    let limit = params.limit.unwrap_or(HITS).clamp(1, MOST_HITS);
    let started = Instant::now();
    let mut at = params.from.clone();
    let mut counted = at.as_ref().map_or(0, |at| at.counted);
    let mut hits = Vec::new();
    let mut scanned = Scanned::default();
    let mut read = 0;
    let mut complete = false;
    'reading: while read < STEP_BYTES && started.elapsed() < STEP_TIME && !rows.superseded() {
        let wanted = match (&at, older) {
            (None, true) => PageOf::Before(u64::MAX),
            (None, false) => PageOf::From(0),
            (Some(at), true) if at.part.is_empty() => PageOf::Before(at.event),
            (Some(at), false) if at.part.is_empty() => PageOf::After(at.event),
            (Some(at), true) => PageOf::Before(at.event.saturating_add(1)),
            (Some(at), false) => PageOf::From(at.event),
        };
        let (batch, last) = rows.batch(wanted, BATCH_BYTES)?;
        if batch.is_empty() {
            complete = true;
            break;
        }
        for row in batch {
            let row = with_trees(row);
            let Some(event) = row.from_event else {
                continue;
            };
            read += serde_json::to_vec(&row).map_or(0, |json| json.len());
            scanned.from_event = Some(scanned.from_event.map_or(event, |seen| seen.min(event)));
            scanned.to_event = Some(scanned.to_event.map_or(event, |seen| seen.max(event)));
            let mut places = places(&row, &query);
            if older {
                places.reverse();
            }
            for (part, (start, end), text) in places {
                let here = FindAt {
                    event,
                    part: part.clone(),
                    offset: start,
                    counted,
                };
                if at
                    .as_ref()
                    .is_some_and(|at| !beyond(&row, &here, at, older))
                {
                    continue;
                }
                counted += 1;
                if !params.count_only {
                    hits.push(Hit {
                        row: row.id.clone(),
                        from_event: event,
                        part,
                        range: [start, end],
                        preview: preview(&text, start, end),
                    });
                }
                at = Some(FindAt { counted, ..here });
                if hits.len() >= limit {
                    break 'reading;
                }
            }
            at = Some(FindAt {
                event,
                part: String::new(),
                offset: 0,
                counted,
            });
        }
        if last {
            complete = true;
            break;
        }
    }
    let next = (!complete).then_some(at).flatten();
    Ok(FindAnswer {
        hits,
        next,
        scanned,
        counted: Counted {
            matches: counted,
            complete,
        },
    })
}

/// Every place `query` is in `row`'s text parts, in reading order: its part, its range and
/// the part's searchable text.
fn places(row: &ChatMessage, query: &Query) -> Vec<(String, (usize, usize), String)> {
    let mut found = Vec::new();
    for part in &row.parts {
        let (ChatPart::Text { id, .. }, Some(text)) = (part, searchable(part)) else {
            continue;
        };
        for range in matches(&text, query) {
            found.push((id.clone(), range, text.clone()));
        }
    }
    found
}

/// Whether `here` lies past `at` in the direction the find reads, so it was not yet answered.
fn beyond(row: &ChatMessage, here: &FindAt, at: &FindAt, older: bool) -> bool {
    if here.event != at.event {
        return (here.event < at.event) == older;
    }
    if at.part.is_empty() {
        return false;
    }
    let place = |part: &str| {
        row.parts
            .iter()
            .position(|each| matches!(each, ChatPart::Text { id, .. } if id == part))
    };
    let (mine, theirs) = (place(&here.part), place(&at.part));
    match mine.cmp(&theirs) {
        std::cmp::Ordering::Equal => (here.offset < at.offset) == older && here.offset != at.offset,
        order => (order == std::cmp::Ordering::Less) == older,
    }
}

/// The words around a hit, a few either side.
fn preview(text: &str, start: usize, end: usize) -> String {
    let units: Vec<u16> = text.encode_utf16().collect();
    let shown = |range: std::ops::Range<usize>| String::from_utf16_lossy(&units[range]);
    let before = shown(0..start.min(units.len()));
    let hit = shown(start.min(units.len())..end.min(units.len()));
    let after = shown(end.min(units.len())..units.len());
    let lead: String = before
        .chars()
        .rev()
        .take(PREVIEW_CHARS)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let tail: String = after.chars().take(PREVIEW_CHARS).collect();
    let line = format!("{lead}{hit}{tail}").replace('\n', " ");
    let cut_before = before.chars().count() > PREVIEW_CHARS;
    let cut_after = after.chars().count() > PREVIEW_CHARS;
    format!(
        "{}{line}{}",
        if cut_before { "…" } else { "" },
        if cut_after { "…" } else { "" }
    )
}
