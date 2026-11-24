use std::collections::{BTreeSet, HashSet};
use std::path::Path;
use std::sync::Arc;

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

/// A chat of `agent_id` at work in `dir`.
fn chat_in(core: &Arc<Core>, agent_id: &str, dir: &Path) -> impl Sized {
    let launch = ChatLaunch {
        agent_id: agent_id.into(),
        provider: "claude".into(),
        cwd: dir.to_path_buf(),
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
    core.chats()
        .insert(core, launch, Origin::default(), None)
        .unwrap()
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
    let main_top = sikemux_process::files::real_path(main.path()).unwrap();
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
    let _chat = chat_in(&core, "rolo", own.path());
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

/// @Tsuruno ran `sed -i` on `app/src/ui/mod.rs` in its own checkout while @Okita, in another, ran
/// a command naming its own `hosts/mod.rs`. The lines became shared between the two, so
/// @Tsuruno's commit left them out and did not build.
#[tokio::test(flavor = "multi_thread")]
async fn a_command_naming_its_own_file_of_the_same_name_is_not_a_writer_elsewhere() {
    let core = Core::new(Default::default(), None).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let project = sikemux_process::files::real_path(dir.path())
        .unwrap()
        .join("sikemux");
    let copies = project.with_extension("worktrees");
    let project = project.to_string_lossy().into_owned();
    let agents = ["tsuruno", "okita"];
    let mut chats = Vec::new();
    for agent_id in agents {
        let own = copies.join(format!("feat-{agent_id}"));
        std::fs::create_dir_all(&own).unwrap();
        git_init(&own);
        let starting = Starting {
            agent_id,
            project: project.clone(),
            provider: None,
            resume_id: None,
        };
        core.names().give(starting, HashSet::new());
        chats.push(chat_in(&core, agent_id, &own));
    }
    let run = |agent: &str, command: &str| {
        let own = copies.join(format!("feat-{agent}"));
        let command = format!("cd {} && {command}", own.display());
        let update = json!({ "sessionUpdate": "tool_call", "toolCallId": "run-1",
            "kind": "execute", "status": "in_progress", "rawInput": { "command": command } });
        core.collisions().saw(agent, &own, &update);
    };
    run("tsuruno", "sed -i '' 's/a/b/' app/src/ui/mod.rs");
    run(
        "okita",
        "cargo check; cat crates/sikemux-core/src/server/hosts/mod.rs",
    );
    let place = copies.join("feat-tsuruno/app/src/ui/mod.rs");
    let written = Written {
        at: epoch_ms(),
        exact: false,
    };

    let (shells, _) = core
        .collisions()
        .writers(&core, &[], &project, &place, written);
    assert_eq!(shells, BTreeSet::from(["tsuruno".to_owned()]));
}
