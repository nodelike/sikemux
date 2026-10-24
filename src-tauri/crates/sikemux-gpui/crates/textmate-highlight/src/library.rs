use std::collections::{BTreeMap, HashSet};
use std::error::Error;
use std::io::Read;
use std::sync::Mutex;

use flate2::read::GzDecoder;
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
        if let Some((id, _)) = index
            .grammars
            .iter()
            .find(|(_, entry)| entry.at + entry.size > grammars.len())
        {
            return Err(format!("grammar {id} lies outside the bundle").into());
        }
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
        for name in self.index.grammars[id]
            .requires
            .iter()
            .map(String::as_str)
            .chain([id])
        {
            if loaded.contains(name) {
                continue;
            }
            let entry = self.index.grammars.get(name)?;
            let mut source = Vec::new();
            GzDecoder::new(&self.grammars[entry.at..entry.at + entry.size])
                .read_to_end(&mut source)
                .ok()?;
            self.highlighter.load_language(name, source).ok()?;
            loaded.insert(name.to_owned());
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
