#![allow(unused_imports)]

use crate::server::edits::footer::known;
use crate::server::edits::land::{apply, steps};
use crate::server::edits::owners::{git, run};
use crate::server::edits::push::line;
use crate::server::remote_state::RemoteParts;
use crate::server::repo_state::RepoParts;
use crate::server::trust_ask::TrustAsk;
use crate::server::Core;
use crate::tasks::config::{self, ConfigLoad, GenerateConfig};
use crate::trailers::{self, Agent, Commit};
use std::path::Path;
use std::time::Duration;
use uuid::Uuid;

use crate::server::edits::generate::*;
use crate::server::edits::tests_git::git_in;
use crate::server::ServerCore;

/// On 2026-11-07 the check of a slice in this project kept refusing with "sikemux.json is not
/// trusted yet" after the person trusted it: the grant named the project's file, and this
/// looked one up for the integration checkout's, which nobody can be asked about.
#[tokio::test]
async fn a_trusted_file_runs_from_the_integration_checkout_and_a_changed_one_does_not() {
    let root = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(root.path()).unwrap();
    let main = root.join("project");
    std::fs::create_dir(&main).unwrap();
    git_in(&main, &["init", "-q", "-b", "main"]);
    git_in(&main, &["config", "user.name", "t"]);
    git_in(&main, &["config", "user.email", "t@t"]);
    let settings = |command: &str| {
        format!(
            r#"{{"version":1,"generate":{{"command":"{command}","outputs":["generated.txt"]}}}}"#
        )
    };
    std::fs::write(
        main.join("sikemux.json"),
        settings("cp schema.txt generated.txt"),
    )
    .unwrap();
    std::fs::write(main.join("schema.txt"), "two\n").unwrap();
    std::fs::write(main.join("generated.txt"), "one\n").unwrap();
    git_in(&main, &["add", "."]);
    git_in(&main, &["commit", "-qm", "base"]);
    let head = git_in(&main, &["rev-parse", "HEAD"]);
    let checkout = root.join("integration");
    git_in(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            &checkout.to_string_lossy(),
            &head,
        ],
    );
    git_in(&checkout, &["config", "user.name", "t"]);
    git_in(&checkout, &["config", "user.email", "t@t"]);
    let core = Core::new(Default::default(), None).unwrap();
    assert_eq!(
        regenerate(&core, &main, &checkout, &head, &[]).unwrap_err(),
        UNTRUSTED
    );
    let ConfigLoad::Valid { fingerprint, .. } = config::load(&main) else {
        panic!("the project's file is valid");
    };
    let project = main.to_string_lossy().into_owned();
    crate::server::tasks::trust_project(&core, project, fingerprint, None)
        .await
        .unwrap();

    let made = regenerate(&core, &main, &checkout, &head, &[])
        .unwrap()
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(main.join("generated.txt")).unwrap(),
        "two\n"
    );

    std::fs::write(
        main.join("sikemux.json"),
        settings("echo changed > generated.txt"),
    )
    .unwrap();
    git_in(&main, &["add", "sikemux.json"]);
    git_in(&main, &["commit", "-qm", "changes the command"]);
    let changed = git_in(&main, &["rev-parse", "HEAD"]);
    assert_ne!(changed, made);
    git_in(&checkout, &["checkout", "-q", "--detach", &changed]);
    assert_eq!(
        regenerate(&core, &main, &checkout, &changed, &[]).unwrap_err(),
        UNTRUSTED
    );
    assert!(trust_needed(&core, &main, &checkout).is_some());
    assert_eq!(
        std::fs::read_to_string(main.join("generated.txt")).unwrap(),
        "two\n"
    );
}
