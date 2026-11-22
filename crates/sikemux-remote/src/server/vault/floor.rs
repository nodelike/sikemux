//! The oldest a vault may be when this host reads it again, so a server cannot hand back an
//! earlier generation of its key, an earlier revision of an entry, or an entry it removed.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Whether a vault at `generation` is no older than the one this host last read, at `held`.
pub fn takes_generation(held: i64, generation: i64) -> bool {
    generation >= held
}

/// Whether an entry at `revision` is no older than the copy this host holds, at `held`, and
/// newer than the last revision the vault removed it at, `ended`.
pub fn takes_entry(held: Option<i64>, ended: Option<i64>, revision: i64) -> bool {
    held.is_none_or(|held| revision >= held) && ended.is_none_or(|ended| revision > ended)
}

/// Raises each entry's mark in `ended` to the revision it was removed at.
pub fn end<Id: AsRef<str>>(
    ended: &mut BTreeMap<String, i64>,
    gone: impl IntoIterator<Item = (Id, i64)>,
) {
    for (id, revision) in gone {
        let mark = ended.entry(id.as_ref().to_owned()).or_default();
        *mark = revision.max(*mark);
    }
}

/// The newest of the account vault this host has read: the generation of its key, each
/// entry's revision, and the last revision of each entry the vault lists as removed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Floor {
    pub generation: i64,
    pub revisions: BTreeMap<String, i64>,
    pub ended: BTreeMap<String, i64>,
}

impl Floor {
    pub fn takes_generation(&self, generation: i64) -> bool {
        takes_generation(self.generation, generation)
    }

    /// An account vault never lists a removed entry again, so any mark in `ended` refuses it.
    pub fn takes_entry(&self, id: &str, revision: i64) -> bool {
        !self.ended.contains_key(id) && takes_entry(self.revisions.get(id).copied(), None, revision)
    }

    /// Moves up to a read of the vault at `generation`, in which `opened` are the entries that
    /// opened at that generation and `ended` the entries it lists as removed.
    pub fn saw<Opened: AsRef<str>, Ended: AsRef<str>>(
        &mut self,
        generation: i64,
        opened: impl IntoIterator<Item = (Opened, i64)>,
        ended: impl IntoIterator<Item = (Ended, i64)>,
    ) {
        self.generation = self.generation.max(generation);
        for (id, revision) in opened {
            self.wrote(id.as_ref(), revision);
        }
        self.end(ended);
    }

    pub fn wrote(&mut self, id: &str, revision: i64) {
        let mark = self.revisions.entry(id.to_owned()).or_insert(revision);
        *mark = revision.max(*mark);
    }

    pub fn end<Id: AsRef<str>>(&mut self, gone: impl IntoIterator<Item = (Id, i64)>) {
        end(&mut self.ended, gone);
        let ended = &self.ended;
        self.revisions.retain(|id, _| !ended.contains_key(id));
    }
}
