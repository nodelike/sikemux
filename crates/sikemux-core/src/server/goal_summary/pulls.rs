//! Where the pull requests a goal names stand, asked of the code host as its page opens. An
//! agent's last word on a pull request goes stale the moment it merges, so a merged or closed one
//! becomes a moment of its own, and the summary is written again when one changes. Only pull
//! requests the goal's cards hold or its moments name are looked up, and a lookup that fails
//! leaves the state unknown rather than failing the summary.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use sikemux_view::slice::{SourceKind, SourceLink};

use super::moments::{fact, Gathered};
use crate::server::codehost::repo;
use crate::server::Core;
use crate::sessions::asked::prose_of;

/// The most pull requests of one goal looked up.
const MAX_PULLS: usize = 8;
const LOOKUP_TIME: Duration = Duration::from_secs(10);
/// What comes before a pull request's number in what people write.
const SAID: [&str; 4] = ["pr #", "pr ", "pull request #", "pull request "];

fn digits_after(text: &str, at: usize) -> Option<u64> {
    let digits: String = text[at..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

/// The pull requests `text` names: those it calls one, and addresses in the repository `slug`.
fn named_in(text: &str, slug: Option<&str>) -> Vec<u64> {
    let text = text.to_lowercase();
    let mut named = Vec::new();
    for said in SAID {
        for (at, _) in text.match_indices(said) {
            let starts_word = !text[..at].ends_with(|c: char| c.is_alphanumeric());
            named.extend(digits_after(&text, at + said.len()).filter(|_| starts_word));
        }
    }
    if let Some(slug) = slug {
        for address in sikemux_view::chat::PULL_PATHS
            .map(|address| format!("{}{address}", slug.to_lowercase()))
        {
            for (at, _) in text.match_indices(&address) {
                named.extend(digits_after(&text, at + address.len()));
            }
        }
    }
    named
}

/// The pull requests `gathered` names, the latest named first, each with when it was last named.
fn named(gathered: &Gathered, slug: Option<&str>) -> Vec<(u64, u64)> {
    let mut named: Vec<(u64, u64)> = Vec::new();
    let in_moments = (gathered.moments.iter().rev()).flat_map(|moment| {
        (named_in(&moment.source.text, slug).into_iter()).map(|number| (number, moment.source.at))
    });
    for (number, at) in gathered.pulls.iter().copied().chain(in_moments) {
        if named.len() < MAX_PULLS && !named.iter().any(|(known, _)| *known == number) {
            named.push((number, at));
        }
    }
    named
}

fn written_at(found: &Value, key: &str) -> Option<u64> {
    let at = time::OffsetDateTime::parse(
        found.get(key)?.as_str()?,
        &time::format_description::well_known::Rfc3339,
    );
    u64::try_from(at.ok()?.unix_timestamp_nanos() / 1_000_000).ok()
}

/// Adds where each pull request `gathered` names stands, unless it is still open.
pub(super) async fn add(core: &Arc<Core>, project: &str, gathered: &mut Gathered) {
    let found = tokio::time::timeout(LOOKUP_TIME, repo::of_project(core, project)).await;
    let found = found.ok().and_then(Result::ok);
    let slug = found.as_ref().map(repo::Repo::slug);
    for (number, named_at) in named(gathered, slug.as_deref()) {
        let answer = match &found {
            Some(found) => {
                let asked = found.call("pull", json!({ "number": number }));
                tokio::time::timeout(LOOKUP_TIME, asked)
                    .await
                    .ok()
                    .and_then(Result::ok)
            }
            None => None,
        };
        let Some(answer) = answer else {
            let words = format!("Sikemux could not read the state of pull request #{number}.");
            let link = SourceLink::Card {
                card: gathered.card,
            };
            gathered.insert(
                fact(SourceKind::PullRequest, named_at, None, &words, link),
                None,
            );
            continue;
        };
        let state = answer["state"].as_str().unwrap_or("open");
        let verb = match state {
            "merged" => "was merged",
            "closed" => "was closed without merging",
            _ => continue,
        };
        let title = prose_of(answer["title"].as_str().unwrap_or_default());
        let at = written_at(&answer, "mergedAt")
            .or_else(|| written_at(&answer, "closedAt"))
            .or_else(|| written_at(&answer, "updatedAt"))
            .unwrap_or(named_at);
        let words = format!("Pull request #{number} ({title}) {verb}.");
        let link = match answer["url"].as_str() {
            Some(url) => SourceLink::Url {
                url: url.to_owned(),
            },
            None => SourceLink::Card {
                card: gathered.card,
            },
        };
        gathered.insert(
            fact(SourceKind::PullRequest, at, None, &words, link),
            Some(words),
        );
    }
}
