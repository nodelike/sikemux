use std::collections::{BTreeMap, HashMap, HashSet};
use std::error::Error;
use std::io::{self, Read};
use std::sync::Mutex;

use irosashi::{Highlighter, HighlighterBuilder, Session};
use serde::Deserialize;

#[derive(Deserialize)]
struct Index {
    grammars: BTreeMap<String, Entry>,
    aliases: BTreeMap<String, String>,
    roles: Vec<(String, String, Option<String>)>,
}

#[derive(Deserialize)]
struct Entry {
    requires: Vec<String>,
    at: usize,
    size: usize,
}

/// A bundle of grammars: an index naming each grammar, the grammars it needs and where
/// its compressed source sits in `grammars`.
pub struct Library {
    index: Index,
    grammars: &'static [u8],
    highlighter: Highlighter,
    loaded: Mutex<HashSet<String>>,
}

impl Library {
    pub fn new(
        index: &[u8],
        grammars: &'static [u8],
    ) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let index: Index = serde_json::from_slice(index)?;
        Ok(Self {
            index,
            grammars,
            highlighter: HighlighterBuilder::without_bundled_assets()
                .embedded_injections_only(true)
                .build()?,
            loaded: Mutex::default(),
        })
    }

    /// The grammar a language id or one of its aliases names.
    pub fn language(&self, name: &str) -> Option<&str> {
        match self.index.grammars.get_key_value(name) {
            Some((id, _)) => Some(id),
            None => self.index.aliases.get(name).map(String::as_str),
        }
    }

    /// A tokenizer for `language`, unpacking its grammars and every grammar they need
    /// the first time.
    pub fn session(&self, language: &str) -> Option<Session> {
        let id = self.language(language)?;
        let mut loaded = self
            .loaded
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut order: Vec<&str> = Vec::new();
        let mut needed: Vec<(&str, &Entry)> = Vec::new();
        for name in self
            .index
            .grammars
            .get(id)?
            .requires
            .iter()
            .map(String::as_str)
            .chain([id])
        {
            if loaded.contains(name) || order.contains(&name) {
                continue;
            }
            order.push(name);
            needed.push((name, self.index.grammars.get(name)?));
        }
        if !needed.is_empty() {
            needed.sort_unstable_by_key(|(_, entry)| entry.at);
            let mut decoder = zstd::stream::read::Decoder::new(self.grammars).ok()?;
            let mut cursor = 0usize;
            let mut sources: HashMap<&str, Vec<u8>> = HashMap::with_capacity(needed.len());
            for (name, entry) in needed {
                let gap = entry.at.checked_sub(cursor)?;
                if gap > 0 {
                    let skipped =
                        io::copy(&mut (&mut decoder).take(gap as u64), &mut io::sink()).ok()?;
                    if skipped != gap as u64 {
                        return None;
                    }
                    cursor = entry.at;
                }
                let mut source = vec![0u8; entry.size];
                decoder.read_exact(&mut source).ok()?;
                cursor = cursor.checked_add(entry.size)?;
                sources.insert(name, source);
            }
            for name in order {
                let source = sources.remove(name)?;
                self.highlighter.load_language(name, source).ok()?;
                loaded.insert(name.to_owned());
            }
        }
        drop(loaded);
        self.highlighter.session(id).ok()
    }

    /// The bundle's theme rules: a scope selector, the role it gives and its font style.
    pub fn theme_rules(&self) -> impl Iterator<Item = (&str, &str, Option<&str>)> {
        self.index
            .roles
            .iter()
            .map(|(selector, role, font)| (selector.as_str(), role.as_str(), font.as_deref()))
    }
}
