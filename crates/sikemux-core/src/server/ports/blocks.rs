//! The short name and the ten ports each checkout the core cuts keeps from the moment it is
//! cut until it is removed, kept across restarts in `checkouts.json`.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use sikemux_process::sync::Locked;

use crate::server::kept::Kept;
use crate::tasks::checkout::{name_for, MAX_PORT_NAMES};

const FILE: &str = "checkouts.json";
pub(crate) const FIRST_PORT: u16 = 21_000;
pub(crate) const LAST_PORT: u16 = 30_999;
pub(crate) const BLOCK: u16 = MAX_PORT_NAMES as u16;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Held {
    pub path: PathBuf,
    /// The project's own checkout, which names are unique within.
    pub repo: PathBuf,
    pub name: String,
    pub first_port: u16,
}

#[derive(Default, Serialize, Deserialize)]
struct Book {
    checkouts: Vec<Held>,
}

pub(crate) struct Blocks {
    book: Mutex<Book>,
    kept: Kept,
}

impl Blocks {
    pub(crate) fn new(data_dir: Option<&Path>) -> Self {
        let kept = Kept::new(data_dir, FILE);
        Self {
            book: Mutex::new(kept.load()),
            kept,
        }
    }

    /// The checkout at `path`'s name and ports, given now when it has none. A block something
    /// already listens in is passed over. `in_use` says whether a port is taken; `kept` whether
    /// a checkout whose folder is gone is still someone's, when the blocks run out.
    pub(crate) fn reserve(
        &self,
        repo: &Path,
        path: &Path,
        branch: &str,
        in_use: impl Fn(u16) -> bool,
        kept: impl Fn(&Path) -> bool,
    ) -> Result<Held, String> {
        let mut book = self.book.locked();
        if let Some(held) = book.checkouts.iter().find(|held| held.path == path) {
            return Ok(held.clone());
        }
        let first_port = match free_block(&book, &in_use) {
            Some(port) => port,
            None => {
                book.checkouts
                    .retain(|held| held.path.exists() || kept(&held.path));
                free_block(&book, &in_use).ok_or(
                    "every checkout's ports are given out; remove checkouts whose work has landed",
                )?
            }
        };
        let taken: Vec<&str> = (book.checkouts.iter())
            .filter(|held| held.repo == repo)
            .map(|held| held.name.as_str())
            .collect();
        let held = Held {
            path: path.to_owned(),
            repo: repo.to_owned(),
            name: name_for(branch, &taken),
            first_port,
        };
        book.checkouts.push(held.clone());
        self.kept.save(&*book);
        Ok(held)
    }

    /// The checkout `cwd` lies in, when the core gave it ports.
    pub(crate) fn of(&self, cwd: &Path) -> Option<Held> {
        let book = self.book.locked();
        (book.checkouts.iter())
            .filter(|held| cwd.starts_with(&held.path))
            .max_by_key(|held| held.path.as_os_str().len())
            .cloned()
    }

    /// The checkout whose ports `port` is one of.
    pub(crate) fn holding(&self, port: u16) -> Option<String> {
        let book = self.book.locked();
        (book.checkouts.iter())
            .find(|held| (held.first_port..held.first_port + BLOCK).contains(&port))
            .map(|held| held.name.clone())
    }

    /// The checkout at `path` is gone; its name and ports are free again.
    pub(crate) fn free(&self, path: &Path) {
        let mut book = self.book.locked();
        let before = book.checkouts.len();
        book.checkouts.retain(|held| held.path != path);
        if book.checkouts.len() != before {
            self.kept.save(&*book);
        }
    }
}

fn free_block(book: &Book, in_use: &impl Fn(u16) -> bool) -> Option<u16> {
    (FIRST_PORT..=LAST_PORT - BLOCK + 1)
        .step_by(BLOCK as usize)
        .filter(|first| !book.checkouts.iter().any(|held| held.first_port == *first))
        .find(|first| !(*first..*first + BLOCK).any(in_use))
}
