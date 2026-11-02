//! What the person typed in their saved sessions, for finding the rules they
//! keep repeating. Only their own messages are kept, never the agent's, and a
//! message that looks pasted, says too little, or may hold a secret is dropped
//! here, so it never leaves the core.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use rayon::prelude::*;

use super::recent::{provider_hits, Hit, PageScan, RecentProvider};
use super::search::{Hear, Scope, Speaker};
use crate::agent_catalog::AgentKind;
use crate::text::clipped;

/// The most sessions of one CLI looked at.
const MAX_CANDIDATES: usize = 4_000;
/// The newest sessions read.
const MAX_SESSIONS: usize = 300;
/// What all kept messages may hold together, and one session's.
const MAX_BYTES: usize = 120 * 1024;
const MAX_SESSION_BYTES: usize = 8 * 1024;
const MAX_MESSAGE_CHARS: usize = 600;
/// A message longer than this, or with more lines, was pasted rather than said.
const MAX_SAID_CHARS: usize = 1500;
const MAX_SAID_LINES: usize = 12;
/// Fewer words than this only say go ahead.
const MIN_WORDS: usize = 3;
/// Sessions read in parallel between checks of the time left.
const BATCH: usize = 48;

/// Words that, followed by `=` or `:`, give a secret's value.
const SECRET_NAMES: [&str; 11] = [
    "password",
    "passwd",
    "passphrase",
    "secret",
    "token",
    "api_key",
    "apikey",
    "api-key",
    "private_key",
    "access_key",
    "credential",
];
/// How kinds of keys and tokens begin.
const SECRET_STARTS: [&str; 15] = [
    "AKIA",
    "ASIA",
    "AIza",
    "sk-",
    "sk_live_",
    "rk_live_",
    "pk_live_",
    "ghp_",
    "gho_",
    "ghs_",
    "github_pat_",
    "glpat-",
    "xoxb-",
    "xoxp-",
    "eyJ",
];
/// A run of letters and digits this long is taken for a key.
const MIN_KEY_CHARS: usize = 32;

/// What the person typed in one session, oldest first.
#[derive(Default)]
struct Typed {
    messages: Vec<String>,
    bytes: usize,
}

impl Hear for Typed {
    fn push(&mut self, by: Speaker, text: &str) {
        if by != Speaker::Person || self.bytes >= MAX_SESSION_BYTES {
            return;
        }
        if let Some(said) = said(text) {
            self.bytes += said.len();
            self.messages.push(said);
        }
    }
}

/// `text` on one line, cut to what is kept, when the person said it rather
/// than pasted it and it holds nothing that may be a secret.
fn said(text: &str) -> Option<String> {
    let text = text.trim();
    let pasted = text.starts_with('<')
        || text.contains("```")
        || text.chars().count() > MAX_SAID_CHARS
        || text.lines().count() > MAX_SAID_LINES;
    if pasted || text.split_whitespace().count() < MIN_WORDS || may_hold_secret(text) {
        return None;
    }
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    Some(clipped(&line, MAX_MESSAGE_CHARS))
}

/// Whether `text` may hold a secret: a private key, a token of a known kind, a
/// value given to a name that says it is secret, a line of an environment
/// file, an address with a password in it, or a long run of letters and digits.
fn may_hold_secret(text: &str) -> bool {
    if text.contains("-----BEGIN") || text.contains("://") && text.contains('@') {
        return true;
    }
    if text.lines().any(sets_variable) {
        return true;
    }
    let lower = text.to_lowercase();
    let gives_value = |name: &str| {
        lower.match_indices(name).any(|(at, _)| {
            let after = lower[at + name.len()..].trim_start_matches([' ', '"', '\'']);
            after.starts_with(['=', ':'])
        })
    };
    if SECRET_NAMES.iter().any(|name| gives_value(name)) {
        return true;
    }
    text.split(|c: char| !(c.is_ascii_alphanumeric() || "_-+/=".contains(c)))
        .any(|token| {
            let known =
                token.len() >= 16 && SECRET_STARTS.iter().any(|start| token.starts_with(start));
            let long = token.len() >= MIN_KEY_CHARS
                && token.chars().any(|c| c.is_ascii_digit())
                && token.chars().any(|c| c.is_ascii_alphabetic());
            known || long
        })
}

/// A line such as `export API_URL=...`, as an environment file holds.
fn sets_variable(line: &str) -> bool {
    let line = line.trim_start();
    let line = line.strip_prefix("export ").unwrap_or(line);
    let Some((name, value)) = line.split_once('=') else {
        return false;
    };
    name.len() >= 3
        && name.starts_with(|c: char| c.is_ascii_uppercase())
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && !value.trim().is_empty()
}

fn typed(hit: &Hit) -> Typed {
    let mut typed = Typed::default();
    if let Some(agent) = AgentKind::from_name(hit.agent) {
        agent.ops().prose(&hit.found, &mut typed);
    }
    typed
}

/// What the person typed in the saved sessions `scope` takes in, a list per
/// session, newest session first, read until `budget` runs out.
pub(crate) fn asked(
    providers: &[RecentProvider],
    scope: &Scope,
    budget: Duration,
) -> Vec<Vec<String>> {
    let deadline = Instant::now() + budget;
    let exclude = HashSet::new();
    let mut seen = HashSet::new();
    let mut in_scope: HashMap<String, bool> = HashMap::new();
    let mut hits: Vec<Hit> = providers
        .par_iter()
        .flat_map_iter(|provider| {
            let scan = PageScan::all(provider.agent, None, MAX_CANDIDATES, &exclude);
            provider_hits(provider, &scan)
        })
        .collect::<Vec<_>>()
        .into_iter()
        .filter(|hit| seen.insert((hit.agent, hit.found.session.id.clone())))
        .filter(|hit| {
            *in_scope
                .entry(hit.found.project.clone())
                .or_insert_with(|| scope(&hit.found.project))
        })
        .collect();
    hits.sort_unstable_by_key(|hit| Reverse(hit.at_ms));
    hits.truncate(MAX_SESSIONS);
    let mut sessions = Vec::new();
    let mut bytes = 0;
    for batch in hits.chunks(BATCH) {
        if Instant::now() >= deadline {
            break;
        }
        let read: Vec<Typed> = batch.par_iter().map(typed).collect();
        for typed in read.into_iter().filter(|typed| !typed.messages.is_empty()) {
            if bytes + typed.bytes > MAX_BYTES {
                return sessions;
            }
            bytes += typed.bytes;
            sessions.push(typed.messages);
        }
    }
    sessions
}
