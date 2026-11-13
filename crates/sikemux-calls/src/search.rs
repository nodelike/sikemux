use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SearchOptions {
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default)]
    pub whole_word: bool,
    #[serde(default)]
    pub is_regex: bool,
    /// Glob pattern restricting which paths participate (empty = all).
    #[serde(default)]
    pub include: String,
    /// Glob pattern excluding paths (in addition to the built-in denylist).
    #[serde(default)]
    pub exclude: String,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ReplaceFile {
    pub path: String,
    pub match_count: usize,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ReplaceError {
    pub path: String,
    pub reason: String,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ReplaceResults {
    pub files: Vec<ReplaceFile>,
    pub file_count: usize,
    pub match_count: usize,
    pub errors: Vec<ReplaceError>,
    pub dry_run: bool,
    pub elapsed_ms: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplaceParams {
    pub project: String,
    pub query: String,
    pub replace: String,
    #[serde(default)]
    pub options: SearchOptions,
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
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

/// Where `fileWindow` looks: `before` and `after` lines around `line`.
#[derive(Serialize, Deserialize)]
pub struct WindowParams {
    pub path: String,
    pub line: u32,
    pub before: u32,
    pub after: u32,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchParams {
    pub project: String,
    pub query: String,
    #[serde(default)]
    pub options: SearchOptions,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct SearchRange {
    pub start: u32,
    pub end: u32,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub line: u32,
    pub text: String,
    #[serde(default)]
    pub truncated_text: bool,
    pub ranges: Vec<SearchRange>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct SearchFile {
    pub path: String,
    pub matches: Vec<SearchHit>,
}

/// One item of a search's stream: `{ "file": … }` for each file as it is found, then
/// `{ "summary": … }`.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SearchFound {
    File(SearchFile),
    Summary(SearchSummary),
}

/// How a search went, sent after every file it found.
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SearchSummary {
    pub file_count: usize,
    pub match_count: usize,
    pub truncated: bool,
    /// True when the search was superseded by a newer one mid-walk and
    /// returned early. The frontend should treat results as partial.
    #[serde(default)]
    pub cancelled: bool,
    pub elapsed_ms: u64,
}
