#![allow(unused_imports)]

use crate::git::Git;
use crate::server::edits::{line_count, read_text, Edit, Lines};
use crate::server::repo_state::RepoParts;
use crate::server::workspace_state::WorkspaceCore;
use crate::server::workspace_state::WorkspaceParts;
use crate::server::Core;
use sikemux_process::sync::Locked;
use sikemux_view::owners::{CommitLines, LineNamed as Named};
use sikemux_view::owners::{
    FileOwners, LineAuthor as Author, LineNote as Note, LineRemoval as Removal, LineRun as Run,
    Owners,
};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::server::edits::owners::*;
use crate::server::names::Starting;
use crate::server::ServerCore;

/// `git_commit` refused a named file over lines of an agent long closed, with "Ask them with
/// agent_send", because the names the core keeps of a closed agent counted as it being open.
#[test]
fn a_closed_agents_lines_block_nobody_and_a_live_agents_do() {
    let core = Core::new(Default::default(), None).unwrap();
    let give = |agent_id: &str, session: &str| {
        let starting = Starting {
            agent_id,
            project: "/project".to_owned(),
            provider: None,
            resume_id: None,
        };
        let kept = ["closed", "first", "again"].map(str::to_owned);
        core.names().give(starting, HashSet::from(kept));
        core.names().note_session(agent_id, session);
    };
    give("closed", "gone");
    give("first", "taken-up");
    give("again", "taken-up");
    let edit = |agent_id: &str| Edit {
        at: 0,
        agent_id: agent_id.to_owned(),
        shared: Vec::new(),
        name: None,
        cli: "claude".to_owned(),
        model: None,
        feed: String::new(),
        turn: None,
        tool_call_id: String::new(),
        path: String::new(),
        lines: Lines::default(),
        total: None,
    };
    let open = HashSet::from(["again".to_owned()]);
    assert!(core.names().get("closed").is_some());
    assert!(!author(&core, &edit("closed"), &open).open);
    assert!(author(&core, &edit("again"), &open).open);
    assert!(author(&core, &edit("first"), &open).open);
}
