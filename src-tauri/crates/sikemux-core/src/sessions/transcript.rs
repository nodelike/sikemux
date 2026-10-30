//! Where an agent writes a saved session's transcript, under the account
//! folder it keeps it in.

use std::fs;
use std::path::{Path, PathBuf};

use crate::agent_catalog::AgentKind;

const MAX_SESSION_ID_CHARS: usize = 200;

/// Whether `id` can name a session's file without leading out of its folder.
pub fn valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_SESSION_ID_CHARS
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The transcript of session `id` under the account folder `root`, looked for
/// in `cwd`'s project first. Other agents keep their history in shapes Sikemux
/// does not read line by line.
pub fn find(kind: AgentKind, root: &Path, cwd: Option<&str>, id: &str) -> Option<PathBuf> {
    if !valid_session_id(id) {
        return None;
    }
    kind.ops().transcript(root, cwd, id)
}

pub(super) fn written(path: PathBuf) -> Option<PathBuf> {
    fs::metadata(&path)
        .is_ok_and(|meta| meta.is_file() && meta.len() > 0)
        .then_some(path)
}
