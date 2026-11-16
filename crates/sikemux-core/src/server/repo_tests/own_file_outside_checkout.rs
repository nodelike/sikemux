use std::collections::{BTreeSet, HashSet};
use std::path::Path;

use serde_json::json;
use sikemux_process::clock::epoch_ms;
use sikemux_wire::protocol::{ChatLaunch, PermissionMode};

use crate::git::Git;
use crate::server::chat::Origin;
use crate::server::chat_state::ChatParts;
use crate::server::collisions::Written;
use crate::server::names::Starting;
use crate::server::repo_state::RepoParts;
use crate::server::workspace_state::WorkspaceParts;
use crate::server::{Core, ServerCore};

fn git_init(dir: &Path) {
    Git::quick(dir).trimmed(&["init", "-q"]).unwrap();
}

/// @Rolo, in a checkout of its own, ran `sed -i` on a mockup it had written in the main
/// checkout, kept there only because docs/research is ignored. The core left @Rolo out for
/// working elsewhere and told it @Kaede, busy with a command of its own, changed its lines.
#[tokio::test(flavor = "multi_thread")]
async fn an_agent_editing_its_own_file_outside_its_checkout_is_its_writer() {
    let core = Core::new(Default::default(), None).unwrap();
    let main = tempfile::tempdir().unwrap();
    let own = tempfile::tempdir().unwrap();
    git_init(main.path());
    git_init(own.path());
    let main_top = main.path().canonicalize().unwrap();
    let project = main_top.to_string_lossy().into_owned();
    for agent_id in ["rolo", "kaede"] {
        let starting = Starting {
            agent_id,
            project: project.clone(),
            provider: None,
            resume_id: None,
        };
        core.names().give(starting, HashSet::new());
    }
    let launch = ChatLaunch {
        agent_id: "rolo".into(),
        provider: "claude".into(),
        cwd: own.path().to_path_buf(),
        program: "/bin/false".into(),
        args: Vec::new(),
        env: Default::default(),
        mcp_servers: Vec::new(),
        resume_id: None,
        permission_mode: PermissionMode::Bypass,
        model: None,
        model_provider: None,
        effort: None,
        account: None,
        fallbacks: Vec::new(),
    };
    let _chat = core
        .chats()
        .insert(&core, launch, Origin::default(), None)
        .unwrap();
    let place = main_top.join("dir_b.py");
    std::fs::write(&place, "a\nb\n").unwrap();
    let read = json!({ "sessionUpdate": "tool_call_update", "toolCallId": "read-1",
        "kind": "read", "status": "completed", "locations": [{ "path": place }] });
    core.collisions().saw("rolo", own.path(), &read);
    let run = |agent: &str, command: &str| {
        let update = json!({ "sessionUpdate": "tool_call", "toolCallId": "run-1",
            "kind": "execute", "status": "in_progress", "rawInput": { "command": command } });
        core.collisions().saw(agent, own.path(), &update);
    };
    run(
        "rolo",
        &format!("cd {project} && sed -i '' 's/a/c/' dir_b.py"),
    );
    run("kaede", "cargo test");
    std::fs::write(&place, "c\nb\n").unwrap();
    let written = Written {
        at: epoch_ms(),
        exact: false,
    };

    let (shells, terminals) = core
        .collisions()
        .writers(&core, &[], &project, &place, written);
    assert_eq!(shells, BTreeSet::from(["rolo".to_owned()]));
    assert!(terminals.is_empty());
}
