//! Finding a saved session from a vague memory of it: by its title and by what
//! the person and the agent wrote in it, best matches first.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use rayon::prelude::*;
use serde::Serialize;
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use super::recent::{provider_hits, Hit, PageScan, RecentProvider};
use super::{claude, codex, grok, hermes, muse, opencode, pi, title_cache_stamp, TitleCacheStamp};
use crate::agent_catalog::AgentKind;

/// The most sessions of one CLI a search weighs.
const MAX_CANDIDATES: usize = 4_000;
/// How much of a transcript's start, and then of its end, is read for what was said.
const HEAD_READ: u64 = 4 * 1024 * 1024;
const TAIL_READ: u64 = 1024 * 1024;
/// How much of what was said in a session is kept for matching: its first
/// messages, then its last.
const PROSE_HEAD: usize = 48 * 1024;
const PROSE_TAIL: usize = 16 * 1024;
const MAX_MESSAGE: usize = 4 * 1024;
/// What all sessions' kept prose may hold together.
const MAX_CACHED_BYTES: usize = 24 * 1024 * 1024;
/// Kept prose is dropped once no search has used it for this long.
const IDLE_DROP: Duration = Duration::from_secs(10 * 60);
const MAX_TERMS: usize = 12;
const MAX_SNIPPETS: usize = 2;
const SNIPPET_BEFORE: usize = 60;
const SNIPPET_AFTER: usize = 110;
pub const MAX_LIMIT: usize = 20;
/// Sessions read in parallel between checks of the time left.
const BATCH: usize = 48;

const STOPWORDS: &[&str] = &[
    "a", "about", "an", "and", "are", "as", "at", "be", "but", "by", "can", "chat", "did", "do",
    "for", "from", "had", "has", "have", "how", "i", "in", "is", "it", "its", "me", "my", "of",
    "on", "or", "our", "session", "so", "that", "the", "them", "there", "this", "to", "us", "was",
    "we", "were", "what", "when", "where", "which", "while", "who", "why", "with", "you", "your",
];

/// Whether a session that ran in a folder is one the search looks at.
pub type Scope = Box<dyn Fn(&str) -> bool + Sync>;

pub struct SearchRequest {
    pub query: String,
    pub providers: Vec<RecentProvider>,
    /// Every project when absent.
    pub scope: Option<Scope>,
    pub limit: usize,
    /// How long the search may read transcripts before it answers with what it has.
    pub budget: Duration,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SessionMatch {
    pub cli: &'static str,
    pub session_id: String,
    pub title: String,
    pub project: String,
    /// When it was last written to, in Unix milliseconds.
    pub active_at: u64,
    /// Lines of the conversation the words were found in.
    pub snippets: Vec<String>,
    #[serde(skip)]
    pub score: f32,
    /// The account folder of the CLI that saved it, when not the default one.
    #[serde(skip)]
    pub config_path: Option<String>,
}

#[derive(Serialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub matches: Vec<SessionMatch>,
    /// Sessions whose conversation was read, of `sessions` weighed.
    pub read: usize,
    pub sessions: usize,
    /// Whether the time ran out before every session's conversation was read.
    pub cut_short: bool,
}

pub fn search(request: SearchRequest) -> SearchResult {
    let deadline = Instant::now() + request.budget;
    let terms = Terms::parse(&request.query);
    if terms.needle.is_empty() {
        return SearchResult::default();
    }
    let limit = request.limit.clamp(1, MAX_LIMIT);
    let exclude = HashSet::new();
    let mut seen = HashSet::new();
    let mut hits: Vec<(Hit, &RecentProvider)> = request
        .providers
        .par_iter()
        .flat_map_iter(|provider| {
            let scan = PageScan::all(provider.agent, None, MAX_CANDIDATES, &exclude);
            provider_hits(provider, &scan)
                .into_iter()
                .map(move |hit| (hit, provider))
        })
        .collect::<Vec<_>>()
        .into_iter()
        .filter(|(hit, _)| seen.insert((hit.agent, hit.found.session.id.clone())))
        .collect();
    if let Some(scope) = &request.scope {
        let mut in_scope: HashMap<String, bool> = HashMap::new();
        hits.retain(|(hit, _)| {
            *in_scope
                .entry(hit.found.project.clone())
                .or_insert_with(|| scope(&hit.found.project))
        });
    }
    hits.sort_unstable_by_key(|(hit, _)| std::cmp::Reverse(hit.at_ms));

    let now_ms = epoch_ms();
    let mut scored: Vec<(SessionMatch, Option<Arc<str>>)> = Vec::new();
    let mut read = 0;
    let mut strong = 0;
    let mut next = 0;
    let enough = limit + 5;
    while !terms.words.is_empty()
        && next < hits.len()
        && strong < enough
        && Instant::now() < deadline
    {
        let batch = &hits[next..(next + BATCH).min(hits.len())];
        let found: Vec<Option<Arc<str>>> = batch
            .par_iter()
            .map(|(hit, _)| (Instant::now() < deadline).then(|| prose_of(hit)).flatten())
            .collect();
        for ((hit, provider), prose) in batch.iter().zip(found) {
            read += usize::from(prose.is_some());
            if let Some(scored_match) = terms.score(hit, provider, prose.as_deref(), now_ms) {
                strong += usize::from(scored_match.1);
                scored.push((scored_match.0, prose));
            }
        }
        next += batch.len();
    }
    let cut_short = !terms.words.is_empty() && next < hits.len() && strong < enough;
    for (hit, provider) in &hits[next..] {
        if let Some((title_match, _)) = terms.score(hit, provider, None, now_ms) {
            scored.push((title_match, None));
        }
    }
    scored.sort_by(|a, b| b.0.score.total_cmp(&a.0.score));
    scored.truncate(limit);
    let matches: Vec<SessionMatch> = scored
        .into_iter()
        .map(|(mut found, prose)| {
            found.snippets = prose
                .map(|prose| terms.snippets(&prose))
                .unwrap_or_default();
            found
        })
        .collect();
    remember(&matches);
    schedule_idle_drop();
    SearchResult {
        matches,
        read,
        sessions: hits.len(),
        cut_short,
    }
}

/// The session `session_id` of one of `providers`, in any project.
pub fn locate(providers: &[RecentProvider], session_id: &str) -> Option<SessionMatch> {
    let remembered = remembered().locked().get(session_id).cloned();
    if let Some(found) = remembered.filter(|found| {
        providers
            .iter()
            .any(|provider| provider.agent.as_str() == found.cli)
    }) {
        return Some(found);
    }
    let exclude = HashSet::new();
    let found = providers.par_iter().find_map_any(|provider| {
        let scan = PageScan::all(provider.agent, None, MAX_CANDIDATES, &exclude);
        provider_hits(provider, &scan)
            .into_iter()
            .find(|hit| hit.found.session.id == session_id)
            .map(|hit| SessionMatch {
                cli: hit.agent,
                session_id: hit.found.session.id,
                title: hit.found.session.title,
                project: hit.found.project,
                active_at: hit.at_ms,
                snippets: Vec::new(),
                score: 0.0,
                config_path: provider.config_path.clone(),
            })
    })?;
    remember(std::slice::from_ref(&found));
    Some(found)
}

const MAX_REMEMBERED: usize = 256;

/// What recent searches found, so resuming one of them does not list every session again.
fn remembered() -> &'static Mutex<HashMap<String, SessionMatch>> {
    static FOUND: OnceLock<Mutex<HashMap<String, SessionMatch>>> = OnceLock::new();
    FOUND.get_or_init(|| Mutex::new(HashMap::new()))
}

fn remember(matches: &[SessionMatch]) {
    let mut found = remembered().locked();
    if found.len() + matches.len() > MAX_REMEMBERED {
        found.clear();
    }
    for found_match in matches {
        found.insert(found_match.session_id.clone(), found_match.clone());
    }
}

/// The words looked for, lowercased, the whole query as a phrase, and the
/// whole query as typed, lowercased, which a title may hold anywhere.
struct Terms {
    words: Vec<String>,
    phrase: Option<String>,
    needle: String,
}

impl Terms {
    fn parse(query: &str) -> Self {
        let mut all: Vec<String> = Vec::new();
        for word in query
            .split(|c: char| !c.is_alphanumeric())
            .map(str::to_ascii_lowercase)
            .filter(|word| word.len() >= 2 || word.chars().all(|c| c.is_ascii_digit()))
            .filter(|word| !word.is_empty())
        {
            if !all.contains(&word) {
                all.push(word);
            }
        }
        let meaningful: Vec<String> = all
            .iter()
            .filter(|word| !STOPWORDS.contains(&word.as_str()))
            .cloned()
            .collect();
        let mut words = if meaningful.is_empty() {
            all
        } else {
            meaningful
        };
        words.truncate(MAX_TERMS);
        let phrase = query
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_lowercase();
        let phrase = (words.len() > 1 && phrase.contains(' ')).then_some(phrase);
        let needle = query.trim().to_lowercase();
        Terms {
            words,
            phrase,
            needle,
        }
    }

    /// The session ranked, and whether every word was found in it. Words in
    /// its title, more of the words, more mentions and a recent last write
    /// each count for more. A title holding a word, or the whole query, in
    /// the middle of a word counts as found there, for half as much.
    fn score(
        &self,
        hit: &Hit,
        provider: &RecentProvider,
        prose: Option<&str>,
        now_ms: u64,
    ) -> Option<(SessionMatch, bool)> {
        let title = hit.found.session.title.to_ascii_lowercase();
        let text = prose.map(str::to_ascii_lowercase).unwrap_or_default();
        let whole = title_weight(&hit.found.session.title.to_lowercase(), &self.needle);
        let count = self.words.len();
        let mut matched = 0;
        let mut in_title = 0.0;
        let mut mentions = 0;
        for word in &self.words {
            let titled = title_weight(&title, word);
            let said = mentions_of(&text, word, 10);
            in_title += titled;
            matched += usize::from(titled > 0.0 || said > 0);
            mentions += said;
        }
        let count = count.max(1);
        if whole > 0.0 {
            matched = count;
        }
        if matched == 0 || (count >= 3 && matched * 2 < count) {
            return None;
        }
        let in_title = whole.max(in_title / count as f32);
        let coverage = matched as f32 / count as f32;
        let phrase = self
            .phrase
            .as_deref()
            .is_some_and(|phrase| title.contains(phrase) || text.contains(phrase));
        let age_days = now_ms.saturating_sub(hit.at_ms) as f32 / 86_400_000.0;
        let score = 10.0 * coverage * coverage
            + 4.0 * in_title
            + if phrase { 3.0 } else { 0.0 }
            + 1.5 * (1.0 + mentions as f32).ln() / (1.0 + 10.0 * count as f32).ln()
            + 3.0 / (1.0 + age_days / 14.0);
        Some((
            SessionMatch {
                cli: hit.agent,
                session_id: hit.found.session.id.clone(),
                title: hit.found.session.title.clone(),
                project: hit.found.project.clone(),
                active_at: hit.at_ms,
                snippets: Vec::new(),
                score,
                config_path: provider.config_path.clone(),
            },
            matched == count,
        ))
    }

    /// The lines holding the most of the words, each cut to the stretch around the first.
    fn snippets(&self, prose: &str) -> Vec<String> {
        let mut lines: Vec<(usize, usize, &str)> = prose
            .lines()
            .enumerate()
            .filter_map(|(index, line)| {
                let lower = line.to_ascii_lowercase();
                let found = self
                    .words
                    .iter()
                    .filter(|word| mentions_of(&lower, word, 1) > 0)
                    .count();
                (found > 0).then_some((found, index, line))
            })
            .collect();
        lines.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        lines
            .into_iter()
            .take(MAX_SNIPPETS)
            .map(|(_, _, line)| self.excerpt(line))
            .collect()
    }

    fn excerpt(&self, line: &str) -> String {
        let lower = line.to_ascii_lowercase();
        let at = self
            .words
            .iter()
            .filter_map(|word| first_mention(&lower, word))
            .min()
            .unwrap_or(0);
        let start = floor_boundary(line, at.saturating_sub(SNIPPET_BEFORE));
        let end = floor_boundary(line, (at + SNIPPET_AFTER).min(line.len()));
        let start = if start == 0 {
            0
        } else {
            line[start..end]
                .find(' ')
                .map_or(start, |space| start + space + 1)
        };
        let mut excerpt = String::new();
        if start > 0 {
            excerpt.push('…');
        }
        excerpt.push_str(line[start..end].trim());
        if end < line.len() {
            excerpt.push('…');
        }
        excerpt
    }
}

fn floor_boundary(text: &str, mut index: usize) -> usize {
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// How much `title` holding `needle`, both lowercased, counts: fully at the
/// start of a word, half in the middle of one, nothing when it is not there.
fn title_weight(title: &str, needle: &str) -> f32 {
    if needle.is_empty() || !title.contains(needle) {
        0.0
    } else if first_mention(title, needle).is_some() {
        1.0
    } else {
        0.5
    }
}

/// Where `word` first starts a word in `text`, both lowercased.
fn first_mention(text: &str, word: &str) -> Option<usize> {
    text.match_indices(word)
        .map(|(at, _)| at)
        .find(|at| starts_word(text, *at))
}

/// How many times, up to `cap`, a word in `text` starts with `word`.
fn mentions_of(text: &str, word: &str, cap: usize) -> usize {
    text.match_indices(word)
        .filter(|(at, _)| starts_word(text, *at))
        .take(cap)
        .count()
}

fn starts_word(text: &str, at: usize) -> bool {
    !text[..at]
        .chars()
        .next_back()
        .is_some_and(char::is_alphanumeric)
}

/// What the person and the agent wrote, one message a line: the first
/// messages up to [`PROSE_HEAD`], then the last up to [`PROSE_TAIL`].
#[derive(Default)]
pub(super) struct Prose {
    head: String,
    tail: VecDeque<String>,
    tail_bytes: usize,
}

impl Prose {
    pub(super) fn push(&mut self, text: &str) {
        let mut line = String::new();
        for word in text.split_whitespace() {
            if line.len() + word.len() >= MAX_MESSAGE {
                break;
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        if line.is_empty() {
            return;
        }
        if self.tail.is_empty() && self.head.len() + line.len() < PROSE_HEAD {
            self.head.push_str(&line);
            self.head.push('\n');
            return;
        }
        self.tail_bytes += line.len() + 1;
        self.tail.push_back(line);
        while self.tail_bytes > PROSE_TAIL {
            match self.tail.pop_front() {
                Some(dropped) => self.tail_bytes -= dropped.len() + 1,
                None => break,
            }
        }
    }

    fn finish(mut self) -> String {
        for line in self.tail {
            self.head.push_str(&line);
            self.head.push('\n');
        }
        self.head
    }
}

/// Reads a transcript's lines, its start and then its end when it is too
/// long to read whole.
pub(super) fn each_line(path: &Path, mut visit: impl FnMut(&str)) {
    let Ok(mut file) = File::open(path) else {
        return;
    };
    let Ok(len) = file.metadata().map(|metadata| metadata.len()) else {
        return;
    };
    let head_end = len.min(HEAD_READ);
    read_lines(&mut file, 0, head_end, &mut visit);
    if len > head_end {
        let start = len.saturating_sub(TAIL_READ).max(head_end);
        read_lines(&mut file, start, len, &mut visit);
    }
}

fn read_lines(file: &mut File, start: u64, end: u64, visit: &mut dyn FnMut(&str)) {
    if file.seek(SeekFrom::Start(start)).is_err() {
        return;
    }
    let mut reader = BufReader::with_capacity(64 * 1024, file.take(end - start));
    let mut line = Vec::new();
    let mut partial = start > 0;
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        if std::mem::take(&mut partial) {
            continue;
        }
        if let Ok(text) = std::str::from_utf8(&line) {
            visit(text.trim_end());
        }
    }
}

/// What was said in the session, read from its transcript or kept from the last read.
fn prose_of(hit: &Hit) -> Option<Arc<str>> {
    let agent = AgentKind::from_name(hit.agent)?;
    let found = &hit.found;
    let place = found.place.as_deref();
    let id = found.session.id.as_str();
    let stamp = match agent {
        AgentKind::Grok => grok::grok_transcript(place?).map(|path| title_cache_stamp(&path))?,
        AgentKind::Hermes => hermes::hermes_stamp(id)?,
        AgentKind::Opencode | AgentKind::Muse => TitleCacheStamp::at_secs(found.session.mtime),
        _ => title_cache_stamp(place?),
    };
    let key = format!("{}\u{0}{id}", hit.agent);
    if let Some(prose) = cache().get(&key, stamp) {
        return Some(prose);
    }
    let mut prose = Prose::default();
    match agent {
        AgentKind::Claude => each_line(place?, |line| claude::claude_prose(line, &mut prose)),
        AgentKind::Codex => each_line(place?, |line| codex::codex_prose(line, &mut prose)),
        AgentKind::Pi | AgentKind::Omp => {
            each_line(place?, |line| pi::message_prose(line, &mut prose))
        }
        AgentKind::Grok => grok::grok_prose(place?, &mut prose),
        AgentKind::Hermes => hermes::hermes_prose(id, &mut prose),
        AgentKind::Opencode => opencode::opencode_prose(place?, id, &mut prose),
        AgentKind::Muse => muse::muse_prose(id, &mut prose),
    }
    let prose: Arc<str> = prose.finish().into();
    cache().put(key, stamp, prose.clone());
    Some(prose)
}

struct Kept {
    stamp: TitleCacheStamp,
    prose: Arc<str>,
    used: u64,
}

#[derive(Default)]
struct ProseCache {
    kept: HashMap<String, Kept>,
    bytes: usize,
    uses: u64,
    last_search: Option<Instant>,
}

struct SharedCache(Mutex<ProseCache>);

impl SharedCache {
    fn get(&self, key: &str, stamp: TitleCacheStamp) -> Option<Arc<str>> {
        let mut cache = self.0.locked();
        cache.uses += 1;
        let uses = cache.uses;
        let kept = cache.kept.get_mut(key).filter(|kept| kept.stamp == stamp)?;
        kept.used = uses;
        Some(kept.prose.clone())
    }

    fn put(&self, key: String, stamp: TitleCacheStamp, prose: Arc<str>) {
        let mut cache = self.0.locked();
        cache.uses += 1;
        let used = cache.uses;
        cache.bytes += prose.len();
        if let Some(old) = cache.kept.insert(key, Kept { stamp, prose, used }) {
            cache.bytes -= old.prose.len();
        }
        if cache.bytes > MAX_CACHED_BYTES {
            let mut by_use: Vec<(u64, String)> = cache
                .kept
                .iter()
                .map(|(key, kept)| (kept.used, key.clone()))
                .collect();
            by_use.sort_unstable();
            for (_, key) in by_use {
                if cache.bytes <= MAX_CACHED_BYTES * 3 / 4 {
                    break;
                }
                if let Some(dropped) = cache.kept.remove(&key) {
                    cache.bytes -= dropped.prose.len();
                }
            }
        }
    }
}

fn cache() -> &'static SharedCache {
    static CACHE: OnceLock<SharedCache> = OnceLock::new();
    CACHE.get_or_init(|| SharedCache(Mutex::new(ProseCache::default())))
}

/// Drops the kept prose once searches stop, so a core that searched once
/// does not hold it for good.
fn schedule_idle_drop() {
    static WAITING: AtomicBool = AtomicBool::new(false);
    cache().0.locked().last_search = Some(Instant::now());
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return;
    };
    if WAITING.swap(true, Ordering::AcqRel) {
        return;
    }
    runtime.spawn(async {
        let mut wait = IDLE_DROP;
        loop {
            tokio::time::sleep(wait).await;
            let mut cache = cache().0.locked();
            let idle = cache.last_search.map_or(IDLE_DROP, |last| last.elapsed());
            if idle >= IDLE_DROP {
                *cache = ProseCache::default();
                remembered().locked().clear();
                WAITING.store(false, Ordering::Release);
                return;
            }
            wait = IDLE_DROP - idle;
        }
    });
}
