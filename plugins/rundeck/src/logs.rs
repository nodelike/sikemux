// A run's output via /execution/{id}/output, which Rundeck pages with an offset
// cursor (`offset` + `lastModified`), each line with the step it came from.

use std::collections::VecDeque;

use serde::{Deserialize, Deserializer, Serialize};

use crate::client::{get_json, Session};
use crate::error::RundeckResult;

#[derive(Serialize, Clone, Deserialize)]
pub struct LogChunk {
    pub completed: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_cursor")]
    pub offset: Option<String>,
    #[serde(
        rename = "lastModified",
        default,
        deserialize_with = "deserialize_cursor"
    )]
    pub last_modified: Option<String>,
    #[serde(rename = "execCompleted")]
    pub exec_completed: Option<bool>,
    #[serde(rename = "execState")]
    pub exec_state: Option<String>,
    #[serde(rename = "retryBackoff", default)]
    pub retry_backoff: Option<u64>,
    #[serde(default)]
    pub entries: Vec<LogEntry>,
}

/// A line of a run's output, as Rundeck sends it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct LogEntry {
    pub time: Option<String>,
    pub level: Option<String>,
    pub log: Option<String>,
    pub user: Option<String>,
    /// The step it came from, as `2` or `2/1@node`.
    #[serde(rename = "stepctx")]
    pub step_ctx: Option<String>,
    pub node: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum CursorValue {
    String(String),
    Signed(i64),
    Unsigned(u64),
}

fn deserialize_cursor<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(
        Option::<CursorValue>::deserialize(deserializer)?.map(|value| match value {
            CursorValue::String(value) => value,
            CursorValue::Signed(value) => value.to_string(),
            CursorValue::Unsigned(value) => value.to_string(),
        }),
    )
}

const MAX_LINES: u32 = 2000;
const MOST_EXCERPT_LINES: usize = 2000;
const LONGEST_LINE_CHARS: usize = 400;
const MOST_LINES_SEARCHED: usize = 200_000;

/// The end of an execution's output. `truncated` means earlier lines were left out.
#[derive(Serialize)]
pub struct Excerpt {
    pub lines: Vec<String>,
    pub completed: bool,
    pub truncated: bool,
}

fn text_lines(entries: &[LogEntry]) -> impl Iterator<Item = String> + '_ {
    entries
        .iter()
        .filter_map(|entry| entry.log.as_deref())
        .flat_map(str::lines)
        .map(|line| {
            let mut clipped: String = line.chars().take(LONGEST_LINE_CHARS).collect();
            if clipped.len() < line.len() {
                clipped.push('…');
            }
            clipped
        })
}

/// The last `tail` lines, of those containing `grep` when given, ignoring case.
pub async fn excerpt(
    session: &Session,
    execution_id: u64,
    tail: usize,
    grep: Option<&str>,
) -> RundeckResult<Excerpt> {
    let tail = tail.clamp(1, MOST_EXCERPT_LINES);
    let path = format!("/execution/{execution_id}/output");
    let page = |position: (&'static str, String)| {
        let query = vec![
            ("format", "json".to_string()),
            ("maxlines", MAX_LINES.to_string()),
            position,
        ];
        let path = path.clone();
        async move { get_json::<LogChunk>(session, &path, &query).await }
    };
    let grep = grep.map(str::to_lowercase).filter(|text| !text.is_empty());
    let Some(grep) = grep else {
        let chunk = page(("lastlines", tail.to_string())).await?;
        let lines: Vec<String> = text_lines(&chunk.entries).collect();
        let skip = lines.len().saturating_sub(tail);
        return Ok(Excerpt {
            truncated: lines.len() >= tail,
            completed: chunk.exec_completed.unwrap_or(false),
            lines: lines.into_iter().skip(skip).collect(),
        });
    };
    let mut kept: VecDeque<String> = VecDeque::with_capacity(tail);
    let mut truncated = false;
    let mut searched = 0;
    let mut offset = String::from("0");
    loop {
        let chunk = page(("offset", offset.clone())).await?;
        searched += chunk.entries.len();
        for line in text_lines(&chunk.entries).filter(|line| line.to_lowercase().contains(&grep)) {
            if kept.len() == tail {
                kept.pop_front();
                truncated = true;
            }
            kept.push_back(line);
        }
        let moved = chunk.offset.as_ref().is_some_and(|next| *next != offset);
        if let Some(next) = chunk.offset {
            offset = next;
        }
        let read_all = chunk.entries.is_empty() || chunk.completed.unwrap_or(false) || !moved;
        if read_all || searched >= MOST_LINES_SEARCHED {
            return Ok(Excerpt {
                lines: kept.into(),
                completed: chunk.exec_completed.unwrap_or(false),
                truncated: truncated || !read_all,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_numeric_and_string_output_cursors() {
        let numeric: LogChunk = serde_json::from_str(
            r#"{"completed":false,"offset":12,"lastModified":34,"execCompleted":false,"entries":[]}"#,
        )
        .unwrap();
        let string: LogChunk = serde_json::from_str(
            r#"{"completed":false,"offset":"56","lastModified":"78","execCompleted":false,"entries":[]}"#,
        )
        .unwrap();

        assert_eq!(numeric.offset.as_deref(), Some("12"));
        assert_eq!(numeric.last_modified.as_deref(), Some("34"));
        assert_eq!(string.offset.as_deref(), Some("56"));
        assert_eq!(string.last_modified.as_deref(), Some("78"));
    }
}
