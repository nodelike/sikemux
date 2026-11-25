//! A sent reply's pill, clicked: the chat goes to the passage the reply quotes and marks it for a
//! moment, paging it in first when the rows held don't have it. A passage is known by its words,
//! as a chat opened again may have other row ids than the reply was sent with.

use std::collections::HashSet;

use gpui_kit::{Context, SharedString};
use gpui_transcript::{Passage, Row};
use sikemux_calls::call;
use sikemux_calls::chat::ChatPageParams;
use sikemux_calls::chat_find::{FindAt, FindDirection, FindHit, FindParams};
use sikemux_view::chat::{quote_cut, ChatPart, ChatRole, REPLY_URI};

use super::{Chat, ChatRow};
use crate::core::Target;
use crate::workbench::Workbench;

/// The places with a quote's words that are read for the quote itself.
const MOST_READ: usize = 8;
/// The longest run of a quote's words looked for, and how many of its runs are.
const MOST_WORDS: usize = 100;
const MOST_RUNS: usize = 3;
const HITS: usize = 50;

/// Where a reply's passage is drawn.
struct Found {
    row: String,
    part: String,
    /// It is among the words left out of the part, put back.
    left_out: bool,
    passages: Vec<Passage>,
}

impl Workbench {
    /// Shows what the reply `uri`, sent in `message`, quotes.
    pub(super) fn open_passage(
        &mut self,
        agent: &str,
        message: &str,
        uri: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(chat) = self.chats.open.get_mut(agent) else {
            return;
        };
        let rows = chat.transcript.update(cx, |transcript, cx| {
            let rows = transcript.settled(cx);
            let sent = rows.iter().find(|row| row.id() == message)?;
            let item = sent.message.context.iter().find(|item| item.uri == uri)?;
            let quote = item.quote.clone()?;
            let here = replied_row(uri).and_then(|row| found_in(rows, uri, &quote, Some(row)));
            Some((quote, sent.message.from_event, here))
        });
        let Some((quote, sent_event, here)) = rows else {
            return;
        };
        if let Some(found) = here {
            return chat.show_passage(found, cx);
        }
        let Some(feed) = chat.mark.as_ref().map(|mark| mark.feed.clone()) else {
            return;
        };
        let look = Look {
            target: chat.target.clone(),
            params: FindParams {
                agent_id: agent.to_owned(),
                feed,
                query: String::new(),
                match_case: true,
                whole_word: false,
                from: sent_event.map(|event| FindAt {
                    event,
                    ..FindAt::default()
                }),
                direction: FindDirection::Older,
                limit: Some(HITS),
                count_only: false,
            },
            uri: uri.to_owned(),
            quote,
        };
        let agent = agent.to_owned();
        cx.spawn(async move |this, cx| {
            let found = look.find(&this, &agent, cx).await;
            let _ = this.update(cx, |bench, cx| {
                let note = match found {
                    Ok(true) => return,
                    Ok(false) => crate::toast::info(t!("That passage is no longer in this chat")),
                    Err(error) => crate::toast::error(error),
                };
                bench.notify(note, cx);
            });
        })
        .detach();
    }
}

/// Looking through the whole chat for a quote.
struct Look {
    target: Target,
    params: FindParams,
    uri: String,
    quote: String,
}

impl Look {
    async fn find(
        &self,
        bench: &gpui_kit::WeakEntity<Workbench>,
        agent: &str,
        cx: &mut gpui_kit::AsyncApp,
    ) -> Result<bool, String> {
        let mut read = HashSet::new();
        for words in runs_of(&self.quote) {
            let mut from = self.params.from.clone();
            loop {
                let params = FindParams {
                    query: words.clone(),
                    from,
                    ..self.params.clone()
                };
                let mut answer = self.target.request_typed::<call::ChatFind>(params).await?;
                let replied = replied_row(&self.uri);
                (answer.hits).sort_by_key(|hit| Some(hit.row.as_str()) != replied);
                for hit in answer.hits {
                    if read.len() < MOST_READ
                        && read.insert(hit.row.clone())
                        && self.show(&hit, bench, agent, cx).await?
                    {
                        return Ok(true);
                    }
                }
                from = answer.next.filter(|_| read.len() < MOST_READ);
                if from.is_none() {
                    break;
                }
            }
        }
        Ok(false)
    }

    /// Shows the passage when it is in `hit`'s row, held or paged in, or in the answer that row
    /// is in.
    async fn show(
        &self,
        hit: &FindHit,
        bench: &gpui_kit::WeakEntity<Workbench>,
        agent: &str,
        cx: &mut gpui_kit::AsyncApp,
    ) -> Result<bool, String> {
        let whole = !self.uri.contains('#');
        let held = bench.update(cx, |bench, cx| {
            let chat = bench.chats.open.get_mut(agent)?;
            let found = chat.transcript.update(cx, |transcript, cx| {
                let rows = transcript.settled(cx);
                let row = (!whole).then_some(hit.row.as_str());
                found_in(rows, &self.uri, &self.quote, row)
            })?;
            chat.show_passage(found, cx);
            Some(())
        });
        match held {
            Ok(Some(())) => return Ok(true),
            Ok(None) => {}
            Err(_) => return Ok(true),
        }
        let params = ChatPageParams {
            around: Some(hit.from_event),
            ..ChatPageParams::new(agent, self.params.feed.clone())
        };
        let page = self.target.request_typed::<call::ChatPage>(params).await?;
        let shown = bench.update(cx, |bench, cx| {
            let chat = bench.chats.open.get_mut(agent)?;
            if chat.mark.as_ref().map(|mark| &mark.feed) != Some(&self.params.feed) {
                return None;
            }
            let rows: Vec<ChatRow> = (page.rows.iter())
                .filter(|row| chat.shows(row))
                .cloned()
                .map(ChatRow::new)
                .collect();
            let found = found_in(&rows, &self.uri, &self.quote, None)?;
            chat.around_page(page, cx);
            chat.transcript.update(cx, |transcript, cx| {
                transcript.settled(cx);
            });
            chat.show_passage(found, cx);
            Some(())
        });
        Ok(shown.map_or(true, |shown| shown.is_some()))
    }
}

impl Chat {
    /// Opens what folds the passage away, brings it into view and marks it for a moment.
    fn show_passage(&mut self, found: Found, cx: &mut gpui_kit::App) {
        let transcript = self.transcript.read(cx);
        let Some(row) = transcript.rows().iter().find(|row| row.id() == found.row) else {
            return;
        };
        let index = (row.message.parts.iter()).position(|part| part.id() == found.part);
        let shown = |shown: &[u64]| index.is_some_and(|index| shown.contains(&(index as u64)));
        let team = row.message.team.clone();
        let fold = (row.message.fold.clone()).filter(|fold| team.is_none() && !shown(&fold.shown));
        let team = team.map(|team| (shown(&team.shown), team.id));
        let left_out = found.left_out.then(|| super::rows::left_out_key(row));
        let text = match found.left_out {
            true => format!("{}:{}:left-out", found.row, found.part),
            false => format!("{}:{}", found.row, found.part),
        };
        self.reader.update(cx, |reading, cx| {
            reading.opened.extend(fold.map(|fold| fold.id));
            reading.opened.extend(left_out);
            if let Some((in_view, team)) = team {
                if !in_view || reading.team_open(&team) {
                    reading.teams.insert(team, true);
                }
            }
            reading.unfolded.insert(text.into());
            cx.notify();
        });
        self.transcript.update(cx, |transcript, cx| {
            transcript.list().remeasure();
            transcript.show_passages(
                SharedString::from(found.row),
                &found.part,
                found.passages,
                cx,
            );
        });
    }
}

/// The row a reply was made on, as it was named then.
fn replied_row(uri: &str) -> Option<&str> {
    uri.strip_prefix(REPLY_URI)?.split('#').next()
}

/// Where `rows` hold what the reply `uri` quotes, in `row` alone or the row it was made on first.
fn found_in(rows: &[ChatRow], uri: &str, quote: &str, row: Option<&str>) -> Option<Found> {
    let same = |written: &str| quote_cut(written).trim() == quote.trim();
    let agents = || {
        (rows.iter().enumerate())
            .filter(|(_, held)| held.message.role != ChatRole::User)
            .filter(|(_, held)| row.is_none_or(|row| row == held.id()))
    };
    let Some((_, place)) = uri.split_once('#') else {
        let (ix, _) = (agents())
            .find(|(ix, _)| super::meta::copied(rows, *ix).is_some_and(|whole| same(&whole)))?;
        return whole_answer(rows, ix);
    };
    let place: Option<usize> = place
        .rsplit_once('.')
        .and_then(|(_, place)| place.parse().ok());
    let passage = |held: &ChatRow| {
        texts(held).find_map(|(part, left_out, markdown)| {
            let passages = markdown.passages();
            let at = place
                .filter(|at| passages.get(*at).is_some_and(|there| same(&there.quoted)))
                .or_else(|| passages.iter().position(|there| same(&there.quoted)))?;
            Some(Found {
                row: held.id().to_owned(),
                part: part.to_owned(),
                left_out,
                passages: vec![passages.into_iter().nth(at)?],
            })
        })
    };
    let replied = replied_row(uri);
    (agents().filter(|(_, held)| Some(held.id()) == replied))
        .chain(agents().filter(|(_, held)| Some(held.id()) != replied))
        .find_map(|(_, held)| passage(held))
}

/// The start of the answer whose last row is at `ix`.
fn whole_answer(rows: &[ChatRow], ix: usize) -> Option<Found> {
    let start = &rows.get(ix)?.message.meta.as_ref()?.from;
    let from = rows.iter().position(|row| row.id() == start).unwrap_or(ix);
    (rows.get(from..=ix)?.iter()).find_map(|row| {
        let (part, left_out, markdown) = texts(row).next()?;
        Some(Found {
            row: row.id().to_owned(),
            part: part.to_owned(),
            left_out,
            passages: markdown.whole(),
        })
    })
}

/// The texts of an agent's row, as it draws them and then with what was left out put back.
fn texts(row: &ChatRow) -> impl Iterator<Item = (&str, bool, &gpui_transcript::Markdown)> {
    let parts = || {
        (row.message.parts.iter())
            .filter(|part| matches!(part, ChatPart::Text { .. }))
            .map(ChatPart::id)
    };
    let drawn = parts().filter_map(|part| Some((part, false, &row.doc(part)?.markdown)));
    let restored = parts().filter_map(|part| Some((part, true, &row.restored(part)?.markdown)));
    drawn.chain(restored)
}

/// The longest runs of words in `quote`, as a reader sees them, to look for.
fn runs_of(quote: &str) -> Vec<String> {
    let quote = quote.trim_end_matches('…');
    let rendered = crate::reading::render(quote);
    let mut runs: Vec<String> = (0..rendered.blocks().len())
        .flat_map(|block| rendered.texts(block))
        .flat_map(|text| text.lines().map(opening_run).collect::<Vec<_>>())
        .filter(|run| !run.is_empty())
        .collect();
    runs.sort_by_key(|run| std::cmp::Reverse(run.chars().count()));
    runs.truncate(MOST_RUNS);
    runs
}

/// The first words of `line`, no more than are looked for.
fn opening_run(line: &str) -> String {
    let line = line.trim();
    let Some((cut, _)) = line.char_indices().nth(MOST_WORDS) else {
        return line.to_owned();
    };
    let head = line.get(..cut).unwrap_or(line);
    head.rsplit_once(' ')
        .map_or(head, |(whole, _)| whole)
        .trim_end()
        .to_owned()
}
