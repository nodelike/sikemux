#![cfg(any(target_os = "macos", target_os = "linux"))]

use crate::git::Git;
use crate::server::workspace_state::WorkspaceParts;
use crate::server::worktree::build_output::{clear, left, Outcome, SET_ASIDE};
use crate::server::worktree::cut;
use crate::server::worktree::removal::remove_left;
use crate::server::Core;
use crate::server::ServerCore;
use sikemux_process::command::Child;
use sikemux_process::user_environment::command;
use sikemux_view::host_stats::ClearedFor;
use sikemux_wire::protocol::BuildIdentity;
use std::path::Path;
use std::time::{Duration, SystemTime};

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
        .stdin(sikemux_process::command::Stdio::piped())
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

fn committed(dir: &Path) {
    git(dir, &["config", "user.name", "t"]);
    git(dir, &["config", "user.email", "t@t"]);
    git(dir, &["commit", "-q", "--allow-empty", "-m", "work"]);
}

fn untouched_for_days(tree: &Path) {
    let days_ago = SystemTime::now() - Duration::from_secs(4 * 24 * 60 * 60);
    let record = Git::quick(tree)
        .trimmed(&["rev-parse", "--absolute-git-dir"])
        .unwrap();
    for file in [Path::new(&record).join("logs/HEAD"), tree.join(".git")] {
        let file = std::fs::OpenOptions::new().write(true).open(file).unwrap();
        file.set_modified(days_ago).unwrap();
    }
}

/// The first sweep of checkouts with work left in them took every idle worktree beside a project,
/// and most of those in two projects had been added with `git worktree add`.
#[tokio::test]
async fn a_worktree_sikemux_did_not_make_stays_however_long_it_is_untouched() {
    let parent = tempfile::tempdir().unwrap();
    let main = sikemux_process::files::real_path(parent.path())
        .unwrap()
        .join("app");
    std::fs::create_dir(&main).unwrap();
    git(&main, &["init", "-q", "-b", "main"]);
    committed(&main);
    let by_hand = main.with_file_name("app.worktrees").join("by-hand");
    let target = by_hand.to_str().unwrap();
    git(&main, &["worktree", "add", "-q", "-b", "by-hand", target]);
    let (_, made_here) = cut(&main, "feat/made-here", "HEAD").unwrap();
    for tree in [&by_hand, &made_here] {
        committed(tree);
        untouched_for_days(tree);
    }
    let core = Core::new(BuildIdentity::default(), None).unwrap();
    core.workspaces().project_for(&main).unwrap();

    let taken = left(&core);
    for (repo, tree, work) in &taken {
        remove_left(&core, repo, tree, *work).await;
    }
    assert!(!made_here.exists());
    assert!(taken.iter().all(|(_, tree, _)| *tree != by_hand));

    if let Some((repo, _, work)) = taken.first() {
        remove_left(&core, repo, &by_hand, *work).await;
    }
    assert!(by_hand.join(".git").is_file());
}

/// Landed checkouts of closed agents stayed for days, holding only folders our own builds and
/// Astro had filled.
#[tokio::test]
async fn a_landed_worktree_goes_unless_it_holds_a_file_nothing_can_make_again() {
    let parent = tempfile::tempdir().unwrap();
    let main = sikemux_process::files::real_path(parent.path())
        .unwrap()
        .join("app");
    std::fs::create_dir(&main).unwrap();
    git(&main, &["init", "-q", "-b", "main"]);
    std::fs::write(
        main.join(".gitignore"),
        ".astro/\n.env\ndocs/research/\ngenerated/\n",
    )
    .unwrap();
    git(&main, &["add", "."]);
    committed(&main);
    std::fs::write(main.join(".env"), "KEY=main").unwrap();
    let holding = |branch: &str, files: &[(&str, &str)]| {
        let (_, tree) = cut(&main, branch, "HEAD").unwrap();
        for (file, text) in files {
            let file = tree.join(file);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, text).unwrap();
        }
        untouched_for_days(&tree);
        tree
    };
    let rebuilt = holding(
        "rebuilt",
        &[
            ("docs-site/.astro/types.d.ts", "x"),
            (
                "generated/CACHEDIR.TAG",
                "Signature: 8a477f597d28d172789f06886806bc55\n",
            ),
            ("generated/lib/core.a", "x"),
            (".env", "KEY=main"),
        ],
    );
    let kept = [
        holding("notes", &[("docs/research/plan.md", "x")]),
        holding("env", &[(".env", "KEY=mine")]),
        holding("unmarked", &[("generated/lib/core.a", "x")]),
    ];
    let core = Core::new(BuildIdentity::default(), None).unwrap();
    core.workspaces().project_for(&main).unwrap();

    let taken = left(&core);
    assert_eq!(taken.len(), 4);
    for (repo, tree, work) in &taken {
        remove_left(&core, repo, tree, *work).await;
    }

    assert!(!rebuilt.exists());
    for tree in kept {
        assert!(tree.join(".git").is_file(), "removed {}", tree.display());
    }
}
