#![cfg(any(target_os = "macos", target_os = "linux"))]

use crate::git::Git;
use crate::server::worktree::build_output::{clear, Outcome, SET_ASIDE};
use crate::server::Core;
use crate::server::ServerCore;
use sikemux_process::user_environment::command;
use sikemux_view::host_stats::ClearedFor;
use sikemux_wire::protocol::BuildIdentity;
use std::path::Path;
use std::process::Child;
use std::time::Duration;

const STAND_IN: &str = "SIKEMUX_TEST_STAND_IN";

fn git(dir: &Path, args: &[&str]) {
    Git::quick(dir).trimmed(args).unwrap();
}

fn removed_while(core: &Core, tree: &Path, mut child: Child) -> bool {
    let removed = matches!(
        clear(core, tree, ClearedFor::LowSpace, false),
        Outcome::Removed
    );
    child.kill().unwrap();
    child.wait().unwrap();
    removed
}

/// What runs from `target` in the test below: this test binary, copied there.
#[test]
#[ignore]
fn stands_in_for_a_program_in_target() {
    if std::env::var_os(STAND_IN).is_some() {
        std::thread::sleep(Duration::from_secs(30));
    }
}

/// A tools eval ran `target/debug/sikemux-host` from a worktree whose agent had ended its
/// turn, and making room took the whole `target` from under it.
#[test]
fn a_process_running_from_an_idle_worktree_keeps_its_build_output() {
    let parent = tempfile::tempdir().unwrap();
    let main = parent.path().join("app");
    std::fs::create_dir(&main).unwrap();
    git(&main, &["init", "-q"]);
    std::fs::write(main.join(".gitignore"), "target/\n").unwrap();
    git(&main, &["add", "."]);
    git(
        &main,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "base",
        ],
    );
    let tree = parent.path().join("app.worktrees/feat-eval");
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feat/eval",
            tree.to_str().unwrap(),
        ],
    );
    let debug = tree.join("target/debug");
    std::fs::create_dir_all(&debug).unwrap();
    std::fs::write(tree.join("target/.rustc_info.json"), "{}").unwrap();
    let host = debug.join("sikemux-host");
    std::fs::copy(std::env::current_exe().unwrap(), &host).unwrap();
    let script = tree.join("run.sh");
    std::fs::write(&script, "read line\n").unwrap();
    let core = Core::new(BuildIdentity::default(), None).unwrap();

    let from_target = command(&host)
        .args(["--ignored", "--exact", "--quiet"])
        .arg("server::repo_tests::build_output::stands_in_for_a_program_in_target")
        .env(STAND_IN, "1")
        .current_dir(parent.path())
        .spawn()
        .unwrap();
    let by_program = removed_while(&core, &tree, from_target);
    let in_folder = command("sleep")
        .arg("30")
        .current_dir(&tree)
        .spawn()
        .unwrap();
    let by_folder = removed_while(&core, &tree, in_folder);
    let from_script = command("sh")
        .arg(&script)
        .stdin(std::process::Stdio::piped())
        .current_dir(parent.path())
        .spawn()
        .unwrap();
    let by_argument = removed_while(&core, &tree, from_script);
    let with_nothing = matches!(
        clear(&core, &tree, ClearedFor::LowSpace, false),
        Outcome::Removed
    );

    assert!(!by_program, "removed while its program ran from target");
    assert!(!by_folder, "removed while a process worked in it");
    assert!(!by_argument, "removed while a process ran a script in it");
    assert!(with_nothing);
    assert!(!tree.join("target").exists());
    assert!(std::fs::read_dir(tree.parent().unwrap())
        .unwrap()
        .flatten()
        .all(|entry| !entry.file_name().to_string_lossy().starts_with(SET_ASIDE)));
}
