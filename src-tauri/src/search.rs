// Project-scoped global search backing the Cmd+Shift+F panel.
//
// Built on `grep-searcher` (the same engine ripgrep uses). The walker runs
// in PARALLEL (one worker per logical core) and matched files stream back
// to the frontend through a Tauri `Channel<SearchFile>` as they're found —
// the UI starts painting before the walk finishes.
//
// Cancellation: a monotonic generation counter is bumped on every new
// search; in-flight searches check it between files and bail. This means
// fast-typing on a slow repo doesn't pile up N concurrent walks.
//
// All blocking work happens inside `tauri::async_runtime::spawn_blocking`
// so the Tauri command thread pool stays free for unrelated IPC.

use globset::{Candidate, Glob, GlobMatcher};
use grep_matcher::{Match, Matcher};
use grep_regex::RegexMatcherBuilder;
use grep_searcher::{BinaryDetection, Searcher, Sink, SinkMatch};
use ignore::{DirEntry, ParallelVisitor, ParallelVisitorBuilder, WalkBuilder, WalkState};
use rayon::prelude::*;
use regex::bytes::RegexBuilder as BytesRegexBuilder;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::error::{AppError, AppResult};
use crate::files;

// ---- caps ---------------------------------------------------------------

const MAX_FILES: usize = 1000;
const MAX_PER_FILE: usize = 200;
const MAX_TOTAL_MATCHES: usize = 5000;
const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
/// Match-line text is truncated to this many bytes (with a "…" marker)
/// before being shipped to the frontend. The pane only renders ~120 cols
/// of one line anyway, and 4000-col minified-JS hits can blow IPC payload
/// size by 2-3 orders of magnitude.
const MAX_MATCH_TEXT_BYTES: usize = 400;
const MAX_WINDOW_CONTEXT_LINES: u32 = 500;
const MAX_REPLACE_SCANNED_FILES: usize = 250_000;
const MAX_REPLACE_CANDIDATES: usize = 10_000;

fn read_replace_file(path: &Path) -> std::io::Result<Vec<u8>> {
    let file = fs::File::open(path)?;
    let mut bytes = Vec::with_capacity(file.metadata()?.len().min(MAX_FILE_BYTES) as usize);
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(std::io::Error::other("file grew beyond the replace limit"));
    }
    Ok(bytes)
}

// ---- wire types --------------------------------------------------------

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SearchOptions {
    #[serde(default)]
    case_sensitive: bool,
    #[serde(default)]
    whole_word: bool,
    #[serde(default)]
    is_regex: bool,
    /// Glob pattern restricting which paths participate (empty = all).
    #[serde(default)]
    include: String,
    /// Glob pattern excluding paths (in addition to the built-in denylist).
    #[serde(default)]
    exclude: String,
}

#[derive(Serialize, Clone)]
pub struct SearchRange {
    pub start: u32,
    pub end: u32,
}

#[derive(Serialize, Clone)]
pub struct SearchHit {
    pub line: u32,
    pub text: String,
    /// True when `text` was truncated to fit MAX_MATCH_TEXT_BYTES.
    #[serde(default)]
    pub truncated_text: bool,
    pub ranges: Vec<SearchRange>,
}

#[derive(Serialize, Clone)]
pub struct SearchFile {
    pub path: String,
    pub matches: Vec<SearchHit>,
}

#[derive(Serialize)]
pub struct SearchResults {
    /// Always empty: every matching file was already sent down the `on_file`
    /// channel as the walk found it. Returning them a second time doubled the
    /// IPC payload of a large search.
    pub files: Vec<SearchFile>,
    pub file_count: usize,
    pub match_count: usize,
    pub truncated: bool,
    /// True when the search was superseded by a newer one mid-walk and
    /// returned early. The frontend should treat results as partial.
    #[serde(default)]
    pub cancelled: bool,
    pub elapsed_ms: u64,
}

#[derive(Serialize, Clone)]
pub struct ReplaceFile {
    pub path: String,
    pub match_count: usize,
}

#[derive(Debug, Serialize, Clone)]
pub struct ReplaceError {
    pub path: String,
    pub reason: String,
}

#[derive(Serialize)]
pub struct ReplaceResults {
    pub files: Vec<ReplaceFile>,
    pub file_count: usize,
    pub match_count: usize,
    pub errors: Vec<ReplaceError>,
    /// True when no writes were performed (dry_run).
    pub dry_run: bool,
    pub elapsed_ms: u64,
}

// ---- per-file sink -----------------------------------------------------

struct FileSink<'a, M: Matcher> {
    matches: &'a mut Vec<SearchHit>,
    matcher: &'a M,
    limit: usize,
    hit_limit: &'a mut bool,
}

impl<'a, M: Matcher> Sink for FileSink<'a, M> {
    type Error = std::io::Error;

    fn matched(
        &mut self,
        _searcher: &Searcher,
        mat: &SinkMatch<'_>,
    ) -> Result<bool, std::io::Error> {
        if self.matches.len() >= self.limit {
            *self.hit_limit = true;
            return Ok(false);
        }
        let line = mat.line_number().unwrap_or(0) as u32;
        let raw = mat.bytes();
        let trimmed = strip_eol(raw);

        // Recover in-line match spans first against the untruncated bytes
        // so the offsets are accurate, then potentially clip them along
        // with the displayed text.
        let mut ranges: Vec<SearchRange> = Vec::new();
        let _ = self.matcher.find_iter(trimmed, |m: Match| {
            ranges.push(SearchRange {
                start: m.start() as u32,
                end: m.end() as u32,
            });
            true
        });

        let (text, truncated_text, ranges) = clip_text_and_ranges(trimmed, ranges);

        self.matches.push(SearchHit {
            line,
            text,
            truncated_text,
            ranges,
        });
        Ok(true)
    }
}

fn strip_eol(bytes: &[u8]) -> &[u8] {
    let mut end = bytes.len();
    if end > 0 && bytes[end - 1] == b'\n' {
        end -= 1;
        if end > 0 && bytes[end - 1] == b'\r' {
            end -= 1;
        }
    }
    &bytes[..end]
}

/// Cap the match-line text at MAX_MATCH_TEXT_BYTES. When the first match
/// span is far past the cap, we anchor a sliding window around it so the
/// user still sees the hit. Ranges get adjusted to the new origin and any
/// range that falls outside the window is dropped.
fn clip_text_and_ranges(
    bytes: &[u8],
    ranges: Vec<SearchRange>,
) -> (String, bool, Vec<SearchRange>) {
    if bytes.len() <= MAX_MATCH_TEXT_BYTES {
        let text = String::from_utf8_lossy(bytes).into_owned();
        return (text, false, ranges);
    }
    // Anchor a window starting ~64 bytes before the first hit so the user
    // sees some leading context. Bias rightward only when the hit is past
    // the cap; otherwise start from byte 0.
    let first_hit = ranges.first().map(|r| r.start as usize).unwrap_or(0);
    let window_start = if first_hit > MAX_MATCH_TEXT_BYTES.saturating_sub(64) {
        first_hit.saturating_sub(64)
    } else {
        0
    };
    let window_end = (window_start + MAX_MATCH_TEXT_BYTES).min(bytes.len());

    // Round window edges to char boundaries so from_utf8_lossy stays sane.
    let window_start = floor_char_boundary(bytes, window_start);
    let window_end = floor_char_boundary(bytes, window_end);

    let slice = &bytes[window_start..window_end];
    let mut text = String::with_capacity(slice.len() + 6);
    if window_start > 0 {
        text.push('…');
    }
    text.push_str(&String::from_utf8_lossy(slice));
    if window_end < bytes.len() {
        text.push('…');
    }

    let prefix_len = if window_start > 0 { "…".len() } else { 0 };
    let adjusted: Vec<SearchRange> = ranges
        .into_iter()
        .filter_map(|r| {
            let s = r.start as usize;
            let e = r.end as usize;
            if e <= window_start || s >= window_end {
                return None;
            }
            let new_s = s.saturating_sub(window_start) + prefix_len;
            let new_e = e.min(window_end).saturating_sub(window_start) + prefix_len;
            if new_e > new_s {
                Some(SearchRange {
                    start: new_s as u32,
                    end: new_e as u32,
                })
            } else {
                None
            }
        })
        .collect();

    (text, true, adjusted)
}

/// Round `idx` down to the nearest UTF-8 char boundary inside `bytes`.
/// Standard library's `floor_char_boundary` is nightly-only.
fn floor_char_boundary(bytes: &[u8], mut idx: usize) -> usize {
    if idx >= bytes.len() {
        return bytes.len();
    }
    while idx > 0 && (bytes[idx] & 0b1100_0000) == 0b1000_0000 {
        idx -= 1;
    }
    idx
}

// ---- matcher construction ---------------------------------------------

fn build_matcher(query: &str, opts: &SearchOptions) -> AppResult<grep_regex::RegexMatcher> {
    let raw = if opts.is_regex {
        query.to_string()
    } else {
        regex::escape(query)
    };
    let pattern = if opts.whole_word {
        format!(r"\b{}\b", raw)
    } else {
        raw
    };
    RegexMatcherBuilder::new()
        .case_insensitive(!opts.case_sensitive)
        .build(&pattern)
        .map_err(|e| AppError::Search(e.to_string()))
}

fn build_glob(pat: &str) -> AppResult<Option<GlobMatcher>> {
    let trimmed = pat.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    Glob::new(trimmed)
        .map(|g| Some(g.compile_matcher()))
        .map_err(|e| AppError::Search(e.to_string()))
}

fn build_bytes_regex(query: &str, opts: &SearchOptions) -> AppResult<regex::bytes::Regex> {
    let raw = if opts.is_regex {
        query.to_string()
    } else {
        regex::escape(query)
    };
    let pattern = if opts.whole_word {
        format!(r"\b{}\b", raw)
    } else {
        raw
    };
    BytesRegexBuilder::new(&pattern)
        .case_insensitive(!opts.case_sensitive)
        .multi_line(true)
        .build()
        .map_err(|e| AppError::Search(e.to_string()))
}

// ---- cancellation -------------------------------------------------------
//
// Every search bumps a generation scoped to its repo. Workers loop-check that
// the generation they were spawned under is still current; if not, they bail.
// Scoping by repo lets two project search panes run independently instead of
// cancelling each other.

fn generations() -> &'static dashmap::DashMap<String, AtomicU64> {
    static GENERATIONS: OnceLock<dashmap::DashMap<String, AtomicU64>> = OnceLock::new();
    GENERATIONS.get_or_init(dashmap::DashMap::new)
}

fn bump_generation(repo: &str) -> u64 {
    let entry = generations()
        .entry(repo.to_string())
        .or_insert_with(|| AtomicU64::new(0));
    entry.fetch_add(1, Ordering::SeqCst) + 1
}

fn is_current(repo: &str, gen: u64) -> bool {
    generations()
        .get(repo)
        .map(|entry| entry.load(Ordering::Relaxed) == gen)
        .unwrap_or(false)
}

fn clear_generation(repo: &str, generation: u64) {
    let _ = generations().remove_if(repo, |_, current| {
        current.load(Ordering::Acquire) == generation
    });
}

#[tauri::command]
pub fn project_search_cancel(repo: String) {
    generations().remove(&repo);
}

// ---- walker construction (shared by search + replace) ------------------

fn build_walker(repo: &str) -> ignore::WalkParallel {
    WalkBuilder::new(repo)
        // Honor .gitignore so generated/vendored dirs (and per-project
        // codegen output) don't get walked. should_skip_dir still applies
        // for things never tracked by git (build artifacts, .git itself,
        // editor caches, etc.).
        .hidden(false)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .ignore(true)
        .parents(false)
        .follow_links(false)
        .filter_entry(|entry| {
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if !is_dir {
                return true;
            }
            let name = entry.file_name().to_string_lossy();
            !files::should_skip_dir(&name)
        })
        .build_parallel()
}

/// Slice the absolute path string down to a repo-relative path without
/// allocating a separate to_string_lossy temporary. Returns None for
/// degenerate cases (entry was the repo root itself).
fn repo_relative(repo: &str, path: &Path) -> Option<String> {
    let path_str = path.to_string_lossy();
    let root_len = repo.len() + 1;
    if path_str.len() <= root_len {
        return None;
    }
    Some(path_str[root_len..].to_string())
}

// ---- search command (streaming) ----------------------------------------

#[tauri::command]
pub async fn project_search(
    repo: String,
    query: String,
    options: SearchOptions,
    on_file: tauri::ipc::Channel<SearchFile>,
) -> AppResult<SearchResults> {
    let gen = bump_generation(&repo);

    if query.is_empty() {
        clear_generation(&repo, gen);
        return Ok(SearchResults {
            files: vec![],
            file_count: 0,
            match_count: 0,
            truncated: false,
            cancelled: false,
            elapsed_ms: 0,
        });
    }

    let cleanup_repo = repo.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        run_search(repo, query, options, on_file, gen)
    })
    .await
    .map_err(|e| AppError::Search(format!("join: {e}")))?;
    clear_generation(&cleanup_repo, gen);
    result
}

fn run_search(
    repo: String,
    query: String,
    options: SearchOptions,
    on_file: tauri::ipc::Channel<SearchFile>,
    gen: u64,
) -> AppResult<SearchResults> {
    let started = std::time::Instant::now();
    let matcher = build_matcher(&query, &options)?;
    let include = build_glob(&options.include)?;
    let exclude = build_glob(&options.exclude)?;

    let total_matches = AtomicU64::new(0);
    let file_count = AtomicU64::new(0);
    let truncated = std::sync::atomic::AtomicBool::new(false);
    let cancelled = std::sync::atomic::AtomicBool::new(false);

    struct Builder<'a> {
        matcher: &'a grep_regex::RegexMatcher,
        include: &'a Option<GlobMatcher>,
        exclude: &'a Option<GlobMatcher>,
        repo: &'a str,
        on_file: &'a tauri::ipc::Channel<SearchFile>,
        total_matches: &'a AtomicU64,
        file_count: &'a AtomicU64,
        truncated: &'a std::sync::atomic::AtomicBool,
        cancelled: &'a std::sync::atomic::AtomicBool,
        gen: u64,
    }

    impl<'s, 'a: 's> ParallelVisitorBuilder<'s> for Builder<'a> {
        fn build(&mut self) -> Box<dyn ParallelVisitor + 's> {
            Box::new(Visitor {
                matcher: self.matcher,
                include: self.include,
                exclude: self.exclude,
                repo: self.repo,
                on_file: self.on_file,
                total_matches: self.total_matches,
                file_count: self.file_count,
                truncated: self.truncated,
                cancelled: self.cancelled,
                gen: self.gen,
                searcher: {
                    let mut s = Searcher::new();
                    s.set_binary_detection(BinaryDetection::quit(b'\x00'));
                    s
                },
            })
        }
    }

    struct Visitor<'a> {
        matcher: &'a grep_regex::RegexMatcher,
        include: &'a Option<GlobMatcher>,
        exclude: &'a Option<GlobMatcher>,
        repo: &'a str,
        on_file: &'a tauri::ipc::Channel<SearchFile>,
        total_matches: &'a AtomicU64,
        file_count: &'a AtomicU64,
        truncated: &'a std::sync::atomic::AtomicBool,
        cancelled: &'a std::sync::atomic::AtomicBool,
        gen: u64,
        searcher: Searcher,
    }

    impl<'a> ParallelVisitor for Visitor<'a> {
        fn visit(&mut self, entry: Result<DirEntry, ignore::Error>) -> WalkState {
            // Cheap fast-path bails before any per-entry work.
            if !is_current(self.repo, self.gen) {
                self.cancelled.store(true, Ordering::Relaxed);
                return WalkState::Quit;
            }
            let total = self.total_matches.load(Ordering::Relaxed) as usize;
            let files = self.file_count.load(Ordering::Relaxed) as usize;
            if total >= MAX_TOTAL_MATCHES || files >= MAX_FILES {
                self.truncated.store(true, Ordering::Relaxed);
                return WalkState::Quit;
            }

            let entry = match entry {
                Ok(e) => e,
                Err(_) => return WalkState::Continue,
            };
            let ft = match entry.file_type() {
                Some(t) => t,
                None => return WalkState::Continue,
            };
            if !ft.is_file() {
                return WalkState::Continue;
            }
            let path = entry.path();
            let rel = match repo_relative(self.repo, path) {
                Some(s) => s,
                None => return WalkState::Continue,
            };

            // Globs evaluated against a Candidate built once from the
            // path — avoids re-walking the path string twice.
            let cand = Candidate::new(path);
            if let Some(inc) = self.include {
                if !inc.is_match_candidate(&cand) {
                    return WalkState::Continue;
                }
            }
            if let Some(exc) = self.exclude {
                if exc.is_match_candidate(&cand) {
                    return WalkState::Continue;
                }
            }
            if let Ok(meta) = entry.metadata() {
                if meta.len() > MAX_FILE_BYTES {
                    return WalkState::Continue;
                }
            }

            let mut matches: Vec<SearchHit> = Vec::new();
            let mut hit_limit = false;
            let mut sink = FileSink {
                matches: &mut matches,
                matcher: self.matcher,
                limit: MAX_PER_FILE,
                hit_limit: &mut hit_limit,
            };
            if let Err(e) = self.searcher.search_path(self.matcher, path, &mut sink) {
                // Best-effort: log and continue. A genuinely failing file
                // shouldn't sink the whole search.
                eprintln!("search: {} → {e}", rel);
                return WalkState::Continue;
            }
            if hit_limit {
                self.truncated.store(true, Ordering::Relaxed);
            }
            if matches.is_empty() {
                return WalkState::Continue;
            }

            self.total_matches
                .fetch_add(matches.len() as u64, Ordering::Relaxed);
            self.file_count.fetch_add(1, Ordering::Relaxed);

            let _ = self.on_file.send(SearchFile { path: rel, matches });
            WalkState::Continue
        }
    }

    let mut builder = Builder {
        matcher: &matcher,
        include: &include,
        exclude: &exclude,
        repo: &repo,
        on_file: &on_file,
        total_matches: &total_matches,
        file_count: &file_count,
        truncated: &truncated,
        cancelled: &cancelled,
        gen,
    };
    build_walker(&repo).visit(&mut builder);

    Ok(SearchResults {
        files: Vec::new(),
        file_count: file_count.load(Ordering::Relaxed) as usize,
        match_count: total_matches.load(Ordering::Relaxed) as usize,
        truncated: truncated.load(Ordering::Relaxed),
        cancelled: cancelled.load(Ordering::Relaxed),
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}

// ---- replace command ---------------------------------------------------
//
// Two passes. (1) Parallel walk to find candidate files (same filtering as
// search). (2) Parallel rayon iter over candidates: read, replace, write
// atomically via tempfile. dry_run skips step 2's write/rename so the UI
// can preview the destructive op.

#[tauri::command]
pub async fn project_search_replace(
    repo: String,
    query: String,
    replace: String,
    options: SearchOptions,
    dry_run: bool,
) -> AppResult<ReplaceResults> {
    if query.is_empty() {
        return Ok(ReplaceResults {
            files: vec![],
            file_count: 0,
            match_count: 0,
            errors: vec![],
            dry_run,
            elapsed_ms: 0,
        });
    }

    tauri::async_runtime::spawn_blocking(move || {
        run_replace(repo, query, replace, options, dry_run)
    })
    .await
    .map_err(|e| AppError::Search(format!("join: {e}")))?
}

fn run_replace(
    repo: String,
    query: String,
    replace: String,
    options: SearchOptions,
    dry_run: bool,
) -> AppResult<ReplaceResults> {
    let started = std::time::Instant::now();
    let rewrite_re = build_bytes_regex(&query, &options)?;
    let include = build_glob(&options.include)?;
    let exclude = build_glob(&options.exclude)?;

    // ---- pass 1: find candidate paths in parallel ----
    let candidates: Mutex<Vec<std::path::PathBuf>> = Mutex::new(Vec::new());
    let scanned_files = AtomicUsize::new(0);
    let limit_exceeded = AtomicBool::new(false);

    struct CBuilder<'a> {
        include: &'a Option<GlobMatcher>,
        exclude: &'a Option<GlobMatcher>,
        candidates: &'a Mutex<Vec<std::path::PathBuf>>,
        rewrite_re: &'a regex::bytes::Regex,
        scanned_files: &'a AtomicUsize,
        limit_exceeded: &'a AtomicBool,
    }
    impl<'s, 'a: 's> ParallelVisitorBuilder<'s> for CBuilder<'a> {
        fn build(&mut self) -> Box<dyn ParallelVisitor + 's> {
            Box::new(CVisitor {
                include: self.include,
                exclude: self.exclude,
                candidates: self.candidates,
                rewrite_re: self.rewrite_re,
                scanned_files: self.scanned_files,
                limit_exceeded: self.limit_exceeded,
            })
        }
    }
    struct CVisitor<'a> {
        include: &'a Option<GlobMatcher>,
        exclude: &'a Option<GlobMatcher>,
        candidates: &'a Mutex<Vec<std::path::PathBuf>>,
        rewrite_re: &'a regex::bytes::Regex,
        scanned_files: &'a AtomicUsize,
        limit_exceeded: &'a AtomicBool,
    }
    impl<'a> ParallelVisitor for CVisitor<'a> {
        fn visit(&mut self, entry: Result<DirEntry, ignore::Error>) -> WalkState {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => return WalkState::Continue,
            };
            let ft = match entry.file_type() {
                Some(t) => t,
                None => return WalkState::Continue,
            };
            if !ft.is_file() {
                return WalkState::Continue;
            }
            if self.scanned_files.fetch_add(1, Ordering::AcqRel) >= MAX_REPLACE_SCANNED_FILES {
                self.limit_exceeded.store(true, Ordering::Release);
                return WalkState::Quit;
            }
            let path = entry.path();
            let cand = Candidate::new(path);
            if let Some(inc) = self.include {
                if !inc.is_match_candidate(&cand) {
                    return WalkState::Continue;
                }
            }
            if let Some(exc) = self.exclude {
                if exc.is_match_candidate(&cand) {
                    return WalkState::Continue;
                }
            }
            if let Ok(meta) = entry.metadata() {
                if meta.len() > MAX_FILE_BYTES {
                    return WalkState::Continue;
                }
            }
            // Read once during enrollment so only actual matches enter the
            // retained candidate vector. Matching files are intentionally
            // re-read during rewrite to avoid retaining repository contents.
            let Ok(bytes) = read_replace_file(path) else {
                return WalkState::Continue;
            };
            if std::str::from_utf8(&bytes).is_err() || !self.rewrite_re.is_match(&bytes) {
                return WalkState::Continue;
            }
            if let Ok(mut candidates) = self.candidates.lock() {
                if candidates.len() >= MAX_REPLACE_CANDIDATES {
                    self.limit_exceeded.store(true, Ordering::Release);
                    return WalkState::Quit;
                }
                candidates.push(path.to_path_buf());
            }
            WalkState::Continue
        }
    }

    let mut b = CBuilder {
        include: &include,
        exclude: &exclude,
        candidates: &candidates,
        rewrite_re: &rewrite_re,
        scanned_files: &scanned_files,
        limit_exceeded: &limit_exceeded,
    };
    build_walker(&repo).visit(&mut b);

    if limit_exceeded.load(Ordering::Acquire) {
        return Err(AppError::Search(format!(
            "replace scope exceeds the safety limit of {MAX_REPLACE_SCANNED_FILES} scanned files or {MAX_REPLACE_CANDIDATES} matching files; narrow the path or glob"
        )));
    }

    let candidates = candidates.into_inner().unwrap_or_default();
    let root_len = repo.len() + 1;

    // ---- pass 2: parallel rewrite over candidate paths ----
    // Each worker reads, runs replace_all, and writes atomically into a
    // unique tempfile in the same dir, then renames into place. Errors
    // are collected per-file; we never abort the whole batch.
    let results: Vec<Result<Option<ReplaceFile>, ReplaceError>> = candidates
        .par_iter()
        .map(|path| -> Result<Option<ReplaceFile>, ReplaceError> {
            let rel = path
                .to_string_lossy()
                .get(root_len..)
                .unwrap_or_default()
                .to_string();

            let original = read_replace_file(path).map_err(|e| ReplaceError {
                path: rel.clone(),
                reason: format!("read failed: {e}"),
            })?;
            let permissions = fs::metadata(path)
                .map_err(|e| ReplaceError {
                    path: rel.clone(),
                    reason: format!("metadata failed: {e}"),
                })?
                .permissions();

            // UTF-8 check before letting the byte-regex loose. The walker
            // already drops most binaries (BinaryDetection::quit), but
            // replace operates on the raw bytes so we double-check here.
            if std::str::from_utf8(&original).is_err() {
                return Err(ReplaceError {
                    path: rel,
                    reason: "skipped: file is not valid UTF-8".to_string(),
                });
            }

            // Fast no-match short-circuit: skips both the alloc + rename
            // dance for files that survived globbing but have no hits.
            if !rewrite_re.is_match(&original) {
                return Ok(None);
            }

            // Count + rewrite in one pass: replace_all does the work; we
            // re-count separately via find_iter (cheap on bytes we just
            // matched against in cache) for the per-file count report.
            let match_count = rewrite_re.find_iter(&original).count();
            let rewritten = rewrite_re
                .replace_all(&original, replace.as_bytes())
                .into_owned();
            if rewritten == original {
                return Ok(None);
            }

            if dry_run {
                return Ok(Some(ReplaceFile {
                    path: rel,
                    match_count,
                }));
            }

            persist_rewrite(path, &rel, &original, &rewritten, permissions)?;

            Ok(Some(ReplaceFile {
                path: rel,
                match_count,
            }))
        })
        .collect();

    let mut files = Vec::new();
    let mut errors = Vec::new();
    let mut total_matches = 0usize;
    for r in results {
        match r {
            Ok(Some(f)) => {
                total_matches += f.match_count;
                files.push(f);
            }
            Ok(None) => {}
            Err(e) => errors.push(e),
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));

    Ok(ReplaceResults {
        file_count: files.len(),
        match_count: total_matches,
        files,
        errors,
        dry_run,
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}

fn persist_rewrite(
    path: &Path,
    rel: &str,
    original: &[u8],
    rewritten: &[u8],
    permissions: fs::Permissions,
) -> Result<(), ReplaceError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(parent).map_err(|e| ReplaceError {
        path: rel.to_string(),
        reason: format!("tempfile failed: {e}"),
    })?;
    tmp.as_file()
        .set_permissions(permissions)
        .map_err(|e| ReplaceError {
            path: rel.to_string(),
            reason: format!("preserve permissions failed: {e}"),
        })?;
    tmp.write_all(rewritten).map_err(|e| ReplaceError {
        path: rel.to_string(),
        reason: format!("write failed: {e}"),
    })?;

    // Search/replace is a read-modify-write. Never clobber an editor/save or
    // build that changed the path while this worker prepared its tempfile.
    let current = read_replace_file(path).map_err(|e| ReplaceError {
        path: rel.to_string(),
        reason: format!("concurrent-change check failed: {e}"),
    })?;
    if current != original {
        return Err(ReplaceError {
            path: rel.to_string(),
            reason: "skipped: file changed during replacement".to_string(),
        });
    }

    tmp.persist(path).map_err(|e| ReplaceError {
        path: rel.to_string(),
        reason: format!("rename failed: {}", e.error),
    })?;
    Ok(())
}

// ---- windowed file read ------------------------------------------------
//
// For the search-pane preview: rather than streaming a 5MB source file
// across the IPC bridge so CodeMirror can render 30 visible lines, return
// a window of `before` + `after` lines around `line` (1-based) plus the
// starting line number for the snippet.

#[derive(Serialize)]
pub struct FileWindow {
    pub doc: String,
    /// 1-based line number of the first line in `doc`.
    pub start_line: u32,
    /// Total line count of the file (for "you're seeing X of Y" hints).
    pub total_lines: u32,
    /// True if `doc` was clipped from the head/tail of the file.
    pub clipped_head: bool,
    pub clipped_tail: bool,
}

#[tauri::command]
pub async fn read_file_window(
    path: String,
    line: u32,
    before: u32,
    after: u32,
) -> AppResult<FileWindow> {
    if before > MAX_WINDOW_CONTEXT_LINES || after > MAX_WINDOW_CONTEXT_LINES {
        return Err(AppError::Search(format!(
            "file window context is limited to {MAX_WINDOW_CONTEXT_LINES} lines per side"
        )));
    }
    tauri::async_runtime::spawn_blocking(move || {
        let bytes = crate::fs::read_bounded(Path::new(&path), MAX_FILE_BYTES)?;
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let lines: Vec<&str> = text.lines().collect();
        let total = lines.len() as u32;
        if total == 0 {
            return Ok(FileWindow {
                doc: String::new(),
                start_line: 1,
                total_lines: 0,
                clipped_head: false,
                clipped_tail: false,
            });
        }
        let center = line.saturating_sub(1).min(total.saturating_sub(1));
        let start = center.saturating_sub(before);
        let end = (center + after + 1).min(total);
        let slice = &lines[start as usize..end as usize];
        let doc = slice.join("\n");
        Ok(FileWindow {
            doc,
            start_line: start + 1,
            total_lines: total,
            clipped_head: start > 0,
            clipped_tail: end < total,
        })
    })
    .await
    .map_err(|e| AppError::Search(format!("join: {e}")))?
}

#[cfg(test)]
mod tests {
    use super::persist_rewrite;
    use std::fs;
    use tempfile::tempdir;

    #[cfg(unix)]
    #[test]
    fn replacement_preserves_file_mode() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("script.sh");
        fs::write(&path, b"old\n").expect("write");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o751)).expect("chmod");
        let permissions = fs::metadata(&path).expect("metadata").permissions();

        persist_rewrite(&path, "script.sh", b"old\n", b"new\n", permissions).expect("replace");

        assert_eq!(
            fs::metadata(&path).expect("metadata").permissions().mode() & 0o777,
            0o751
        );
        assert_eq!(fs::read(&path).expect("read"), b"new\n");
    }

    #[test]
    fn replacement_rejects_concurrent_file_change() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("file.txt");
        fs::write(&path, b"changed elsewhere\n").expect("write");
        let permissions = fs::metadata(&path).expect("metadata").permissions();

        let error = persist_rewrite(&path, "file.txt", b"old\n", b"new\n", permissions)
            .expect_err("concurrent change must be rejected");

        assert!(error.reason.contains("changed during replacement"));
        assert_eq!(fs::read(&path).expect("read"), b"changed elsewhere\n");
    }
}
