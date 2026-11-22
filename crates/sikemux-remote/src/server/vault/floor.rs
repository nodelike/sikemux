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

    pub fn takes_entry(&self, id: &str, revision: i64) -> bool {
        takes_entry(
            self.revisions.get(id).copied(),
            self.ended.get(id).copied(),
            revision,
        )
    }

    /// The first entry this host holds that a vault leaves out without listing it as removed, or
    /// lists as removed at a revision older than this host's copy.
    pub fn missing<'a>(
        &'a self,
        listed: &BTreeMap<&str, i64>,
        ended: &BTreeMap<&str, i64>,
    ) -> Option<&'a str> {
        (self.revisions.iter())
            .filter(|(id, _)| !listed.contains_key(id.as_str()))
            .find(|(id, held)| ended.get(id.as_str()).is_none_or(|ended| ended < held))
            .map(|(id, _)| id.as_str())
    }

    /// Moves up to a read of the vault at `generation`: the revision of each entry it lists, and
    /// of each it lists as removed.
    pub fn saw(
        &mut self,
        generation: i64,
        listed: &BTreeMap<&str, i64>,
        ended: &BTreeMap<&str, i64>,
    ) {
        end(
            &mut self.ended,
            ended.iter().map(|(id, revision)| (id, *revision)),
        );
        self.ended.retain(|id, _| !listed.contains_key(id.as_str()));
        self.generation = self.generation.max(generation);
        self.revisions = (listed.iter())
            .map(|(id, revision)| {
                let held = self.revisions.get(*id).copied().unwrap_or(*revision);
                (id.to_string(), held.max(*revision))
            })
            .collect();
    }

    pub fn wrote(&mut self, id: &str, revision: i64) {
        self.revisions.insert(id.to_owned(), revision);
    }
}
