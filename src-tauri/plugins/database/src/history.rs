// The SQL run against each saved database, newest first, so a query can be
// found and run again. One file per database, one JSON line per run.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::{DatabaseError, DatabaseResult};

/// Runs kept per database; older ones are dropped when the file is next tidied.
const KEPT: usize = 500;
/// The file is rewritten to `KEPT` lines once it grows this far past it, not on every run.
const TIDY_AT: usize = KEPT + 100;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Person,
    Agent,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub sql: String,
    /// Milliseconds since 1970.
    pub at: u64,
    pub millis: u64,
    pub ok: bool,
    /// Rows the last statement returned or changed.
    #[serde(default)]
    pub rows: Option<u64>,
    #[serde(default)]
    pub error: Option<String>,
    pub source: Source,
}

#[derive(Deserialize, Debug)]
pub struct HistoryRequest {
    pub id: String,
    #[serde(default)]
    pub search: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

pub fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

fn file(data_dir: &Path, id: &str) -> DatabaseResult<PathBuf> {
    let safe = !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    if !safe {
        return Err(DatabaseError::BadArg(format!(
            "{id} is not a saved database's id"
        )));
    }
    Ok(data_dir.join("history").join(format!("{id}.jsonl")))
}

/// Oldest first, as the file holds them. A line that does not parse is skipped rather than losing the rest.
fn read_all(path: &Path) -> Vec<Entry> {
    std::fs::read_to_string(path)
        .map(|text| {
            text.lines()
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect()
        })
        .unwrap_or_default()
}

fn write_all(path: &Path, entries: &[Entry]) -> DatabaseResult<()> {
    let mut text = String::new();
    for entry in entries {
        text.push_str(&serde_json::to_string(entry)?);
        text.push('\n');
    }
    let staged = path.with_extension("jsonl.tmp");
    std::fs::write(&staged, text)?;
    std::fs::rename(&staged, path)?;
    Ok(())
}

/// Adds a run. Running the same SQL again moves it to the top instead of listing it twice.
pub fn record(data_dir: &Path, id: &str, entry: Entry) -> DatabaseResult<()> {
    let path = file(data_dir, id)?;
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder)?;
    }
    let mut entries = read_all(&path);
    let repeated = entries.last().is_some_and(|last| last.sql == entry.sql);
    if repeated || entries.len() >= TIDY_AT {
        if repeated {
            entries.pop();
        }
        entries.push(entry);
        let keep_from = entries.len().saturating_sub(KEPT);
        return write_all(&path, entries.get(keep_from..).unwrap_or_default());
    }
    let mut appending = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    appending.write_all(format!("{}\n", serde_json::to_string(&entry)?).as_bytes())?;
    Ok(())
}

/// Newest first, those whose SQL holds every word searched for, in any case.
pub fn list(data_dir: &Path, request: &HistoryRequest) -> DatabaseResult<Vec<Entry>> {
    let path = file(data_dir, &request.id)?;
    let words: Vec<String> = request
        .search
        .as_deref()
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_lowercase)
        .collect();
    Ok(read_all(&path)
        .into_iter()
        .rev()
        .filter(|entry| {
            let sql = entry.sql.to_lowercase();
            words.iter().all(|word| sql.contains(word.as_str()))
        })
        .take(request.limit.unwrap_or(KEPT).min(KEPT))
        .collect())
}

pub fn clear(data_dir: &Path, id: &str) -> DatabaseResult<()> {
    match std::fs::remove_file(file(data_dir, id)?) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sikemux-database-history-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn entry(sql: &str, at: u64) -> Entry {
        Entry {
            sql: sql.into(),
            at,
            millis: 3,
            ok: true,
            rows: Some(1),
            error: None,
            source: Source::Person,
        }
    }

    fn all(dir: &Path, search: Option<&str>) -> Vec<String> {
        let request = HistoryRequest {
            id: "p1".into(),
            search: search.map(str::to_string),
            limit: None,
        };
        list(dir, &request)
            .unwrap()
            .into_iter()
            .map(|entry| entry.sql)
            .collect()
    }

    #[test]
    fn runs_come_back_newest_first_and_a_repeat_moves_to_the_top() {
        let dir = scratch("order");
        assert!(all(&dir, None).is_empty());
        record(&dir, "p1", entry("select 1", 1)).unwrap();
        record(&dir, "p1", entry("select 2", 2)).unwrap();
        record(&dir, "p1", entry("select 2", 3)).unwrap();
        assert_eq!(all(&dir, None), vec!["select 2", "select 1"]);
        record(&dir, "p1", entry("select 1", 4)).unwrap();
        assert_eq!(all(&dir, None), vec!["select 1", "select 2", "select 1"]);
    }

    #[test]
    fn search_matches_every_word_in_any_case() {
        let dir = scratch("search");
        record(&dir, "p1", entry("SELECT * FROM orders", 1)).unwrap();
        record(&dir, "p1", entry("select name from customers", 2)).unwrap();
        assert_eq!(all(&dir, Some("from ORDERS")), vec!["SELECT * FROM orders"]);
        assert_eq!(all(&dir, Some("select")).len(), 2);
        assert!(all(&dir, Some("orders customers")).is_empty());
    }

    #[test]
    fn only_the_newest_runs_are_kept() {
        let dir = scratch("kept");
        for at in 0..(TIDY_AT as u64 + 1) {
            record(&dir, "p1", entry(&format!("select {at}"), at)).unwrap();
        }
        let kept = all(&dir, None);
        assert_eq!(kept.len(), KEPT);
        assert_eq!(kept[0], format!("select {TIDY_AT}"));
    }

    #[test]
    fn clearing_forgets_everything_and_odd_ids_are_refused() {
        let dir = scratch("clear");
        record(&dir, "p1", entry("select 1", 1)).unwrap();
        clear(&dir, "p1").unwrap();
        clear(&dir, "p1").unwrap();
        assert!(all(&dir, None).is_empty());
        assert!(matches!(
            record(&dir, "../escape", entry("select 1", 1)),
            Err(DatabaseError::BadArg(_))
        ));
    }
}
