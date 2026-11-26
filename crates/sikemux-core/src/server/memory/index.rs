//! Fast local SQLite FTS5 / BM25 full-text index over Rules, Knowledge, and Standing Decisions.
//! Kept in memory per project and lazily rebuilt whenever the project's memory revision advances,
//! giving sub-millisecond ranked recall with zero extra disk files or crate/DMG bloat.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use rusqlite::{params, Connection};
use sikemux_process::sync::Locked;

const MAX_QUERY_TERMS: usize = 24;

const STOP_WORDS: [&str; 46] = [
    "a", "an", "the", "and", "or", "but", "for", "nor", "not", "so", "yet", "in", "on", "at", "to",
    "of", "by", "as", "is", "it", "be", "am", "are", "was", "were", "been", "this", "that",
    "these", "those", "with", "from", "into", "about", "after", "before", "under", "over", "where",
    "which", "while", "would", "could", "should", "have", "has",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum EntryKind {
    Rule,
    Knowledge,
    Decision,
}

impl EntryKind {
    fn tag(self) -> &'static str {
        match self {
            Self::Rule => "rule",
            Self::Knowledge => "knowledge",
            Self::Decision => "decision",
        }
    }

    fn from_tag(tag: &str) -> Option<Self> {
        match tag {
            "rule" => Some(Self::Rule),
            "knowledge" => Some(Self::Knowledge),
            "decision" => Some(Self::Decision),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct IndexedDoc {
    pub kind: EntryKind,
    pub id: u64,
    pub text: String,
    pub about: String,
    pub why: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SearchHit {
    pub kind: EntryKind,
    pub id: u64,
    pub score: f64,
}

struct CachedProject {
    rev: u64,
    conn: Connection,
}

#[derive(Default)]
pub(crate) struct MemoryIndex {
    projects: Mutex<HashMap<String, CachedProject>>,
}

impl MemoryIndex {
    /// Searches indexed entries for `project` at `rev`, rebuilding the in-memory FTS5 table only
    /// when `rev` changed. Returns hits ranked by BM25 relevance (highest score first).
    pub(crate) fn search(
        &self,
        project: &str,
        rev: u64,
        docs: impl FnOnce() -> Vec<IndexedDoc>,
        kind: Option<EntryKind>,
        query: &str,
        limit: usize,
    ) -> Vec<SearchHit> {
        let Some(fts) = build_fts_query(query) else {
            return Vec::new();
        };
        let mut projects = self.projects.locked();
        let needs_build = projects.get(project).is_none_or(|cached| cached.rev != rev);
        if needs_build {
            let built = build_connection(&docs());
            match built {
                Ok(conn) => {
                    projects.insert(project.to_owned(), CachedProject { rev, conn });
                }
                Err(error) => {
                    eprintln!("sikemux core: memory FTS5 index build failed: {error}");
                    return Vec::new();
                }
            }
        }
        let Some(cached) = projects.get(project) else {
            return Vec::new();
        };
        run_query(&cached.conn, kind, &fts, limit).unwrap_or_default()
    }
}

fn build_connection(docs: &[IndexedDoc]) -> rusqlite::Result<Connection> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch(
        "CREATE VIRTUAL TABLE mem_fts USING fts5(
            kind UNINDEXED,
            entry_id UNINDEXED,
            text,
            about,
            why,
            tokenize='porter unicode61'
        );",
    )?;
    let tx = conn.unchecked_transaction()?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO mem_fts (kind, entry_id, text, about, why) VALUES (?1, ?2, ?3, ?4, ?5)",
        )?;
        for doc in docs {
            stmt.execute(params![
                doc.kind.tag(),
                doc.id as i64,
                doc.text,
                doc.about,
                doc.why,
            ])?;
        }
    }
    tx.commit()?;
    Ok(conn)
}

fn run_query(
    conn: &Connection,
    kind: Option<EntryKind>,
    fts: &str,
    limit: usize,
) -> rusqlite::Result<Vec<SearchHit>> {
    let limit = limit.max(1) as i64;
    let mut hits = Vec::new();
    match kind {
        Some(kind) => {
            let mut stmt = conn.prepare(
                "SELECT kind, entry_id, -bm25(mem_fts, 0.0, 0.0, 1.0, 2.0, 0.6) AS score
                 FROM mem_fts
                 WHERE mem_fts MATCH ?1 AND kind = ?2
                 ORDER BY score DESC
                 LIMIT ?3",
            )?;
            let rows = stmt.query_map(params![fts, kind.tag(), limit], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, f64>(2)?,
                ))
            })?;
            for row in rows.flatten() {
                if let Some(kind) = EntryKind::from_tag(&row.0) {
                    hits.push(SearchHit {
                        kind,
                        id: row.1.max(0) as u64,
                        score: row.2,
                    });
                }
            }
        }
        None => {
            let mut stmt = conn.prepare(
                "SELECT kind, entry_id, -bm25(mem_fts, 0.0, 0.0, 1.0, 2.0, 0.6) AS score
                 FROM mem_fts
                 WHERE mem_fts MATCH ?1
                 ORDER BY score DESC
                 LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![fts, limit], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, f64>(2)?,
                ))
            })?;
            for row in rows.flatten() {
                if let Some(kind) = EntryKind::from_tag(&row.0) {
                    hits.push(SearchHit {
                        kind,
                        id: row.1.max(0) as u64,
                        score: row.2,
                    });
                }
            }
        }
    }
    Ok(hits)
}

/// Sanitizes arbitrary text (including URLs, file paths, `#id`, `@Chief`, punctuation) into a safe
/// FTS5 OR-query with prefix matching on 3+ char terms and exact matching on 2-char domain tokens
/// (`ui`, `ci`, `pr`).
pub(crate) fn build_fts_query(raw: &str) -> Option<String> {
    let mut seen = HashSet::new();
    let mut all_tokens = Vec::new();
    for piece in raw.split(|c: char| !c.is_alphanumeric()) {
        let lower = piece.to_lowercase();
        if lower.chars().count() < 2 {
            continue;
        }
        if seen.insert(lower.clone()) {
            all_tokens.push(lower);
        }
    }
    if all_tokens.is_empty() {
        return None;
    }
    let filtered: Vec<String> = all_tokens
        .iter()
        .filter(|tok| !STOP_WORDS.contains(&tok.as_str()))
        .cloned()
        .collect();
    let chosen = if filtered.is_empty() {
        all_tokens
    } else {
        filtered
    };
    let terms: Vec<String> = chosen
        .into_iter()
        .take(MAX_QUERY_TERMS)
        .map(|tok| {
            if tok.chars().count() >= 3 {
                format!("\"{tok}\"*")
            } else {
                format!("\"{tok}\"")
            }
        })
        .collect();
    Some(terms.join(" OR "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fts5_bm25_recalls_older_relevant_decision_behind_50_newer_ones() {
        let index = MemoryIndex::default();
        let mut docs = vec![IndexedDoc {
            kind: EntryKind::Decision,
            id: 180,
            text: "Design consults, planning and research run on Gemini 4 Neon via Antigravity, never Fable.".into(),
            about: String::new(),
            why: "The person said neon is superior and banned fable.".into(),
        }];
        for i in 181..=240 {
            docs.push(IndexedDoc {
                kind: EntryKind::Decision,
                id: i,
                text: format!("Unrelated UI layout tweak #{i} for padding and border color."),
                about: String::new(),
                why: "Polish pass.".into(),
            });
        }
        let hits = index.search(
            "sikemux",
            1,
            || docs.clone(),
            Some(EntryKind::Decision),
            "should we use neon or fable for browser design review?",
            5,
        );
        assert!(!hits.is_empty());
        assert_eq!(hits[0].id, 180);
    }

    #[test]
    fn fts5_matches_short_domain_tokens_stems_and_punctuation_safely() {
        let index = MemoryIndex::default();
        let docs = vec![
            IndexedDoc {
                kind: EntryKind::Knowledge,
                id: 1,
                text: "MCP gateway exposes tools through sikemux-tools server.".into(),
                about: "crates/sikemux-core/src/server/memory/brief.rs mcp".into(),
                why: String::new(),
            },
            IndexedDoc {
                kind: EntryKind::Knowledge,
                id: 2,
                text: "CI workflow requires green checks before nightly release.".into(),
                about: ".github/workflows/ci.yml ci".into(),
                why: String::new(),
            },
        ];
        let mcp_hits = index.search(
            "sikemux",
            1,
            || docs.clone(),
            Some(EntryKind::Knowledge),
            "check @Chief sikemux://memory mcp server!",
            5,
        );
        assert_eq!(mcp_hits.first().map(|h| h.id), Some(1));

        let ci_hits = index.search(
            "sikemux",
            1,
            || docs.clone(),
            Some(EntryKind::Knowledge),
            "why is ci failing on #296?",
            5,
        );
        assert_eq!(ci_hits.first().map(|h| h.id), Some(2));
    }
}
