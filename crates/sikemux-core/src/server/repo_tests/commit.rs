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
use crate::trailers::{self, Commit};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

use crate::server::edits::commit::*;
use crate::server::edits::tests_git::git_in;
use crate::server::names::{Starting, Worker};
use crate::server::workspace::{AddProjectParams, AddProjectParamsExt};
use crate::server::ServerCore;

#[test]
fn a_path_in_a_folder_of_repositories_is_found_one_level_down() {
    let folder = tempfile::tempdir().unwrap();
    let folder = sikemux_process::files::real_path(folder.path()).unwrap();
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
    let as_paths = |paths: &[String]| paths.iter().map(PathBuf::from).collect::<Vec<_>>();
    assert_eq!(as_paths(&paths), as_paths(&named("api/lib/only.ts")));

    let several = checkout_for(&named("shared.ts")).unwrap_err();
    assert!(
        several.contains("api") && several.contains("web"),
        "{several}"
    );

    let nowhere = checkout_for(&named("missing.ts")).unwrap_err();
    assert!(nowhere.contains("is not in a git checkout"), "{nowhere}");
}

/// A folder of two made-up repositories, `web` and `api`, each pushed to a remote of its own and
/// with a worktree beside it on a branch of its own. The agent's own checkout is web's.
struct TwoRepos {
    _home: tempfile::TempDir,
    folder: PathBuf,
    core: Arc<Core>,
}

impl TwoRepos {
    async fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let folder = sikemux_process::files::real_path(home.path()).unwrap();
        for repo in ["web", "api"] {
            let dir = folder.join(repo);
            std::fs::create_dir(&dir).unwrap();
            git_in(
                &folder,
                &["init", "-q", "--bare", "-b", "main", &format!("{repo}.git")],
            );
            git_in(&dir, &["init", "-q", "-b", "main"]);
            git_in(&dir, &["config", "user.name", "t"]);
            git_in(&dir, &["config", "user.email", "t@t"]);
            std::fs::write(dir.join("a.txt"), "one\n").unwrap();
            git_in(&dir, &["add", "a.txt"]);
            git_in(&dir, &["commit", "-qm", "start"]);
            git_in(
                &dir,
                &["remote", "add", "origin", &format!("../{repo}.git")],
            );
            git_in(&dir, &["push", "-q", "-u", "origin", "main"]);
            let tree = format!("../{repo}.worktrees/feat-{repo}");
            git_in(
                &dir,
                &[
                    "worktree",
                    "add",
                    "-q",
                    "-b",
                    &format!("feat/{repo}"),
                    &tree,
                ],
            );
            let remote = folder.join(format!("{repo}.git"));
            git_in(
                &dir,
                &["remote", "set-url", "origin", &remote.to_string_lossy()],
            );
        }
        let core = Core::new(Default::default(), None).unwrap();
        let open =
            || HashSet::from(["lead", "agent", "second", "third", "stray"].map(str::to_owned));
        for agent_id in ["lead", "stray"] {
            core.names().give(
                Starting {
                    agent_id,
                    project: folder.to_string_lossy().into_owned(),
                    provider: None,
                    resume_id: None,
                },
                open(),
            );
        }
        for agent_id in ["agent", "second", "third"] {
            let worker = Worker {
                agent_id,
                lead: "lead",
                project: None,
                provider: "claude",
                session: None,
                reports: true,
                task: None,
            };
            core.names().hire(worker, open()).unwrap();
        }
        let project =
            crate::server::workspace::add_project(&core, AddProjectParams::at(folder.clone()))
                .await
                .unwrap();
        let draft = serde_json::json!({
            "id": "agent", "title": "claude", "provider": "claude",
            "project": project.id, "cwd": folder.join("web.worktrees/feat-web"),
            "permissionMode": "bypass",
            "worktree": {
                "repo": folder.join("web"), "path": folder.join("web.worktrees/feat-web"),
                "branch": "feat/web", "base": "main", "startSha": ""
            },
        });
        let serde_json::Value::Object(draft) = draft else {
            unreachable!()
        };
        core.workspaces().put_agent_draft(draft).unwrap();
        let lead = serde_json::json!({
            "id": "lead", "title": "claude", "provider": "claude",
            "project": project.id, "cwd": folder, "permissionMode": "bypass",
        });
        let serde_json::Value::Object(lead) = lead else {
            unreachable!()
        };
        core.workspaces().put_agent_draft(lead).unwrap();
        Self {
            _home: home,
            folder,
            core,
        }
    }

    async fn git_commit(&self, params: Value) -> Result<Value, String> {
        self.git_commit_by("agent", params).await
    }

    async fn git_commit_by(&self, agent: &str, params: Value) -> Result<Value, String> {
        let request = HarnessRequest {
            id: "1".into(),
            project: self.folder.to_string_lossy().into_owned(),
            agent_id: Some(agent.into()),
            method: "git.commit".into(),
            params,
        };
        call(&self.core, &request).await
    }

    fn git(&self, repo: &str, args: &[&str]) -> String {
        git_in(&self.folder.join(repo), args).trim().to_owned()
    }

    /// `agent` commits `line` at the end of `file` in api's worktree, and the file's path.
    fn adds(&self, agent: &str, file: &str, line: &str) -> String {
        let api = self.folder.join("api.worktrees/feat-api");
        let path = api.join(file);
        let was = std::fs::read_to_string(&path).unwrap_or_default();
        std::fs::write(&path, format!("{was}{line}\n")).unwrap();
        let made_by = trailers::Agent {
            name: agent.into(),
            cli: "claude".into(),
            cli_version: None,
            model: None,
            sikemux: None,
            session: None,
            chat: agent.into(),
        };
        let unsigned = Commit {
            tree: "",
            parents: &[],
        };
        let footer = trailers::footer(&[made_by], &unsigned, None);
        let message = trailers::append(&format!("Add {line}"), &footer);
        git_in(&api, &["add", file]);
        let tree = git_in(&api, &["write-tree"]);
        let made = ["commit-tree", tree.trim(), "-p", "HEAD", "-m", &message];
        let made = git_in(&api, &made);
        git_in(&api, &["update-ref", "HEAD", made.trim()]);
        path.to_string_lossy().into_owned()
    }

    async fn pushes(&self, agent: &str, file: &str) -> Result<Value, String> {
        let asked = serde_json::json!({ "push": true, "paths": [file] });
        self.git_commit_by(agent, asked).await
    }
}

/// An agent whose own checkout is in one repository pushed work it had made in another, and was
/// answered `"branch": "main"` with nothing pushed and no refusal.
#[tokio::test(flavor = "multi_thread")]
async fn a_push_that_sends_nothing_is_refused_and_one_naming_paths_goes_from_their_checkout() {
    let repos = TwoRepos::new().await;
    let file = repos.adds("agent", "new.txt", "new");

    let refused = repos.git_commit(serde_json::json!({ "push": true })).await;
    let refused = refused.unwrap_err();
    let web = repos.folder.join("web").to_string_lossy().into_owned();
    assert!(
        refused.contains("main") && refused.contains(&web),
        "{refused}"
    );

    let asked = serde_json::json!({ "push": true, "paths": [file] });
    let pushed = repos.git_commit(asked.clone()).await.unwrap();
    let api = repos.folder.join("api.worktrees/feat-api");
    assert_eq!(pushed["pushed"]["branch"], "feat/api");
    assert_eq!(pushed["pushed"]["checkout"], api.to_string_lossy().as_ref());
    assert_eq!(
        repos.git("api.git", &["rev-parse", "feat/api"]),
        repos.git("api.worktrees/feat-api", &["rev-parse", "HEAD"])
    );

    let again = repos.git_commit(asked).await.unwrap_err();
    assert!(again.contains("feat/api"), "{again}");
}

#[tokio::test(flavor = "multi_thread")]
async fn sync_and_land_act_on_the_checkout_of_the_paths_or_refuse() {
    let repos = TwoRepos::new().await;
    let file = repos.adds("agent", "new.txt", "new");
    for repo in ["web", "api"] {
        let dir = repos.folder.join(repo);
        std::fs::write(dir.join("later.txt"), "later\n").unwrap();
        git_in(&dir, &["add", "later.txt"]);
        git_in(&dir, &["commit", "-qm", "later"]);
    }
    let web_was = repos.git("web.worktrees/feat-web", &["rev-parse", "HEAD"]);

    let asked = serde_json::json!({ "sync": true, "paths": [file] });
    let synced = repos.git_commit(asked).await.unwrap();
    let api = repos.folder.join("api.worktrees/feat-api");
    assert_eq!(synced["synced"]["checkout"], api.to_string_lossy().as_ref());
    assert!(api.join("later.txt").exists());
    assert_eq!(
        repos.git("web.worktrees/feat-web", &["rev-parse", "HEAD"]),
        web_was
    );

    let asked = serde_json::json!({ "land": true, "paths": [file] });
    let refused = repos.git_commit(asked).await.unwrap_err();
    let web = repos.folder.join("web.worktrees/feat-web");
    for named in [&api, &web] {
        assert!(refused.contains(&*named.to_string_lossy()), "{refused}");
    }
}

/// Three agents' commits lay interleaved on a branch the remote lacked, so each one's push was
/// told to ask the others to go first.
#[tokio::test(flavor = "multi_thread")]
async fn interleaved_commits_go_whole_with_the_lead_every_refusal_names() {
    let repos = TwoRepos::new().await;
    let authors = ["agent", "second", "third"];
    let mut file = String::new();
    for line in ["one", "two"] {
        for author in authors {
            let line = format!("{author} {line}");
            file = repos.adds(author, "shared.txt", &line);
        }
    }
    let api = repos.folder.join("api.worktrees/feat-api");
    for author in authors {
        let refused = repos.pushes(author, &file).await.unwrap_err();
        assert!(refused.contains("(lead)"), "{refused}");
        assert!(refused.contains(&*api.to_string_lossy()), "{refused}");
        assert!(!refused.contains("Ask them"), "{refused}");
    }
    let above = repos.pushes("second", &file).await.unwrap_err();
    assert!(!above.contains("Add third two"), "{above}");

    let pushed = repos.pushes("lead", &file).await.unwrap();
    assert_eq!(pushed["pushed"]["pushed"].as_array().unwrap().len(), 6);
    assert_eq!(
        repos.git("api.git", &["rev-parse", "feat/api"]),
        repos.git("api.worktrees/feat-api", &["rev-parse", "HEAD"])
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_push_stuck_on_commits_no_lead_shares_names_the_person() {
    let repos = TwoRepos::new().await;
    repos.adds("stray", "shared.txt", "stray");
    let file = repos.adds("agent", "shared.txt", "agent");
    let refused = repos.pushes("agent", &file).await.unwrap_err();
    assert!(refused.contains("the person"), "{refused}");
    assert!(!refused.contains("(lead)"), "{refused}");
}

/// A push asked for while another agent's ran from the same checkout was answered with that
/// one's outcome, which counted its commits as another agent's and sent none of them.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_push_asked_for_during_another_agents_sends_its_own_commits_after() {
    use std::os::unix::fs::PermissionsExt;
    let repos = TwoRepos::new().await;
    let go = repos.folder.join("go");
    let started = repos.folder.join("started");
    let hook = repos.folder.join("api/.git/hooks/pre-push");
    let script = format!(
        "#!/bin/sh\ntouch '{}'\nwhile [ ! -e '{}' ]; do sleep 0.05; done\n",
        started.display(),
        go.display()
    );
    std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
    std::fs::write(&hook, script).unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    let first = repos.adds("agent", "first.txt", "first");
    let second = repos.adds("second", "second.txt", "second");

    let wait_for = |what: &'static str, met: Box<dyn Fn() -> bool>| {
        for _ in 0..400 {
            if met() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("{what}");
    };
    let joined = async {
        tokio::task::block_in_place(|| {
            let started = started.clone();
            wait_for("the first push runs", Box::new(move || started.exists()));
        });
        let joined = repos.pushes("second", &second).await;
        std::fs::write(&go, "").unwrap();
        joined
    };
    let (pushed, joined) = tokio::join!(repos.pushes("agent", &first), joined);
    assert_eq!(
        pushed.unwrap()["pushed"]["pushed"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(joined.unwrap()["stillPushing"].is_string());
    let remote = repos.folder.join("api.git");
    tokio::task::block_in_place(|| {
        let sent =
            move || git_in(&remote, &["log", "--format=%s", "feat/api"]).contains("Add second");
        wait_for("the second agent's commit goes", Box::new(sent));
    });
}
