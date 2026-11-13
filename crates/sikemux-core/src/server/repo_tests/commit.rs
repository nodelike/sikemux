#![allow(unused_imports)]

use crate::cli::protocol::HarnessRequest;
use crate::server::connection::blocking;
use crate::server::edits::committer;
use crate::server::edits::footer::{agent, attributed, known};
use crate::server::edits::named;
use crate::server::edits::owners::{git, run, traced, Diff, FileOwners, Hunk, Owners};
use crate::server::remote_state::RemoteParts;
use crate::server::repo_events::{commit_made, CommitMade};
use crate::server::repo_state::RepoParts;
use crate::server::workspace_state::WorkspaceParts;
use crate::server::Core;
use crate::trailers::Commit;
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

use crate::server::edits::commit::*;
use crate::server::edits::tests_git::git_in;
use crate::server::ServerCore;

#[test]
fn a_path_in_a_folder_of_repositories_is_found_one_level_down() {
    let folder = tempfile::tempdir().unwrap();
    let folder = std::fs::canonicalize(folder.path()).unwrap();
    for repo in ["api", "web"] {
        let repo = folder.join(repo);
        std::fs::create_dir_all(repo.join("lib")).unwrap();
        git_in(&repo, &["init", "-q"]);
        std::fs::write(repo.join("shared.ts"), "").unwrap();
    }
    std::fs::write(folder.join("api/lib/only.ts"), "").unwrap();
    let named = |path: &str| vec![folder.join(path).to_string_lossy().into_owned()];
    let core = Core::new(Default::default(), None).unwrap();
    let checkout_for = |named: &[String]| checkout_for(&core, "agent", &folder, named);

    let (dir, paths) = checkout_for(&named("lib/only.ts")).unwrap();
    assert_eq!(dir, folder.join("api"));
    assert_eq!(paths, named("api/lib/only.ts"));

    let several = checkout_for(&named("shared.ts")).unwrap_err();
    assert!(
        several.contains("api") && several.contains("web"),
        "{several}"
    );

    let nowhere = checkout_for(&named("missing.ts")).unwrap_err();
    assert!(nowhere.contains("is not in a git checkout"), "{nowhere}");
}
