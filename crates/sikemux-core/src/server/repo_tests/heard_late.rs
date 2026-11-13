use std::collections::HashSet;

use serde_json::json;
use sikemux_process::clock::epoch_ms;

use crate::server::collisions::Written;
use crate::server::names::Starting;
use crate::server::repo_state::RepoParts;
use crate::server::workspace_state::WorkspaceParts;
use crate::server::{Core, ServerCore};

/// On a loaded CI runner the core handled an agent's tool call a moment after its quick command
/// had written, so the file was nobody's and the turn read "done, no changes".
#[test]
fn a_file_written_just_before_the_core_hears_of_the_command_is_that_commands() {
    let core = Core::new(Default::default(), None).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().to_string_lossy().into_owned();
    let starting = Starting {
        agent_id: "writer",
        project: project.clone(),
        provider: None,
        resume_id: None,
    };
    core.names().give(starting, HashSet::new());
    let place = dir.path().join("notes.txt");
    let written = Written {
        at: epoch_ms() - 40,
        exact: true,
    };
    let call = |status: &str| {
        let update = json!({ "sessionUpdate": "tool_call", "toolCallId": "run-1",
            "title": "echo hello > notes.txt", "kind": "execute", "status": status });
        core.collisions().saw("writer", dir.path(), &update);
    };

    call("in_progress");
    let (shells, _) = (core.collisions()).writers(&core, &[], &project, &place, written);
    assert!(shells.contains("writer"), "while the command runs");

    call("completed");
    let (shells, _) = (core.collisions()).writers(&core, &[], &project, &place, written);
    assert!(shells.contains("writer"), "once it has ended");
}
