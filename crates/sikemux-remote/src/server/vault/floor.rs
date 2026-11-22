//! The oldest a vault may be when this host reads it again, so a server cannot hand back an
//! earlier generation of its key, an earlier revision of an entry, or an entry it removed.

use std::collections::BTreeMap;

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
