//! The code a worker on another host starts from, and the commits it sends
//! back. The lead's host sends its commit and, as a patch, what it has not
//! committed. The worker's host cuts a worktree of the project when it has it,
//! found by its `origin`, fetching the commit from a git bundle when it lacks
//! it, or keeps a copy of its own in its data folder. The worker's commits come
//! back as a bundle onto a branch named for it, which the lead merges or not.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::super::board::git::{
    branch_exists, git, git_in, git_with, inside, slug, worktrees_folder, SLOW,
};
use super::super::connection::blocking;
use super::super::Core;
use super::link::Received;
use super::name_of;
use super::team::{Role, Tree};

/// More than this of uncommitted changes the lead commits first.
const MAX_PATCH_BYTES: usize = 6 * 1024 * 1024;
const MAX_TIPS: usize = 64;
const CLONES: &str = "host-projects";

/// The lead's checkout, as a worker on another host starts from it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Code {
    /// What the project is called, for a host that keeps a copy of its own.
    pub project: String,
    /// Its `origin`, which a host that has the project finds it by.
    #[serde(default)]
    pub remote: Option<String>,
    pub commit: String,
    #[serde(default)]
    pub branch: Option<String>,
    /// Where the lead works inside the repository.
    #[serde(default)]
    pub inside: String,
    /// What the lead has not committed, as a binary patch on `commit`.
    #[serde(default)]
    pub patch: Option<String>,
}

/// The lead's repository and what a worker elsewhere needs of it.
pub(crate) struct Packed {
    pub repo: PathBuf,
    pub code: Code,
}

/// A file made for the link, removed once it is dropped.
pub(crate) struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        Self(std::env::temp_dir().join(format!("sikemux-{name}-{}", uuid::Uuid::new_v4().simple())))
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn is_commit_id(text: &str) -> bool {
    matches!(text.len(), 40 | 64) && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn has_commit(repo: &Path, commit: &str) -> bool {
    git(repo, &["cat-file", "-e", &format!("{commit}^{{commit}}")]).is_ok()
}

/// What the lead working in `cwd` would hand a worker on another host.
pub(crate) fn package(cwd: &Path) -> Result<Packed, String> {
    let repo = PathBuf::from(
        git(
            cwd,
            &["rev-parse", "--path-format=absolute", "--show-toplevel"],
        )
        .map_err(|_| {
            "your folder is not in a git repository, so its code cannot go to another host"
                .to_owned()
        })?,
    );
    let commit = git(&repo, &["rev-parse", "HEAD"]).map_err(|_| {
        "your repository has no commit yet, so a worker on another host has nothing to start from"
            .to_owned()
    })?;
    let project = repo
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".to_owned());
    let code = Code {
        project,
        remote: git(&repo, &["remote", "get-url", "origin"])
            .ok()
            .filter(|url| !url.is_empty()),
        branch: git(&repo, &["symbolic-ref", "--quiet", "--short", "HEAD"]).ok(),
        inside: inside(cwd, &repo).to_string_lossy().into_owned(),
        patch: uncommitted(&repo)?,
        commit,
    };
    Ok(Packed { repo, code })
}

/// Everything not committed, new files too, as a patch on `HEAD`, worked out
/// in an index of its own so the lead's staging stays as it is.
fn uncommitted(repo: &Path) -> Result<Option<String>, String> {
    let index = git(
        repo,
        &["rev-parse", "--path-format=absolute", "--git-path", "index"],
    )?;
    let scratch = Scratch::new("index");
    if Path::new(&index).exists() {
        std::fs::copy(&index, scratch.path())
            .map_err(|error| format!("could not read the repository's index: {error}"))?;
    }
    let scratch_index = scratch.path().to_string_lossy().into_owned();
    let envs = [("GIT_INDEX_FILE", scratch_index.as_str())];
    git_with(repo, &["add", "--all"], &envs, SLOW)?;
    let patch = Scratch::new("patch");
    let output = format!("--output={}", patch.path().display());
    git_with(
        repo,
        &[
            "diff",
            "--cached",
            "--binary",
            "--no-color",
            &output,
            "HEAD",
        ],
        &envs,
        SLOW,
    )?;
    let text = std::fs::read_to_string(patch.path()).unwrap_or_default();
    if text.len() > MAX_PATCH_BYTES {
        return Err(format!(
            "your uncommitted changes come to {} MB, more than a worker on another host is sent; commit them first",
            text.len() / (1024 * 1024)
        ));
    }
    Ok((!text.trim().is_empty()).then_some(text))
}

/// A bundle of `commit` without what a host holding `tips` already has.
pub(crate) fn bundle(repo: &Path, commit: &str, tips: &[String]) -> Result<Scratch, String> {
    let reference = format!("refs/sikemux/outgoing/{}", uuid::Uuid::new_v4().simple());
    git(repo, &["update-ref", &reference, commit])?;
    let scratch = Scratch::new("bundle");
    let target = scratch.path().to_string_lossy().into_owned();
    let excluded: Vec<String> = tips
        .iter()
        .filter(|tip| is_commit_id(tip) && *tip != commit && has_commit(repo, tip))
        .take(MAX_TIPS)
        .map(|tip| format!("^{tip}"))
        .collect();
    let mut args = vec!["bundle", "create", "--quiet", &target, &reference];
    args.extend(excluded.iter().map(String::as_str));
    let made = git_in(repo, &args, SLOW);
    let _ = git(repo, &["update-ref", "-d", &reference]);
    made?;
    Ok(scratch)
}

/// The one branch tip a bundle carries.
fn bundled_ref(repo: &Path, bundle: &Path) -> Result<String, String> {
    let bundle = bundle.to_string_lossy().into_owned();
    let heads = git(repo, &["bundle", "list-heads", &bundle])?;
    heads
        .lines()
        .find_map(|line| line.split_whitespace().nth(1))
        .filter(|reference| reference.starts_with("refs/"))
        .map(str::to_owned)
        .ok_or_else(|| "the bundle holds no branch".to_owned())
}

/// Fetches what `bundle` holds into `repo`, under `into`.
fn fetch(repo: &Path, bundle: &Path, into: &str) -> Result<(), String> {
    git_in(
        repo,
        &["bundle", "verify", "--quiet", &bundle.to_string_lossy()],
        SLOW,
    )
    .map_err(|error| format!("the bundle does not fit this repository: {error}"))?;
    let reference = bundled_ref(repo, bundle)?;
    let refspec = format!("+{reference}:{into}");
    git_in(
        repo,
        &[
            "fetch",
            "--quiet",
            "--no-write-fetch-head",
            &bundle.to_string_lossy(),
            &refspec,
        ],
        SLOW,
    )
    .map(drop)
}

/// A remote's address with its scheme, user and `.git` left out, so the same
/// repository reads the same however it was cloned.
fn normalized(url: &str) -> String {
    let url = url.trim().trim_end_matches('/');
    let url = url.strip_suffix(".git").unwrap_or(url);
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let rest = match rest.split_once('@') {
        Some((user, host)) if !user.contains('/') => host,
        _ => rest,
    };
    let rest = match rest.split_once(':') {
        Some((host, path)) if !path.starts_with(|c: char| c.is_ascii_digit()) => {
            format!("{host}/{path}")
        }
        _ => rest.to_owned(),
    };
    rest.to_lowercase()
}

/// The checkout of a project here whose `origin` is `remote`.
fn project_with(core: &Core, remote: &str) -> Option<PathBuf> {
    let wanted = normalized(remote);
    core.workspaces
        .project_paths()
        .into_iter()
        .find_map(|project| {
            let repo = git(
                &project,
                &["rev-parse", "--path-format=absolute", "--show-toplevel"],
            )
            .ok()?;
            let repo = PathBuf::from(repo);
            let origin = git(&repo, &["remote", "get-url", "origin"]).ok()?;
            (normalized(&origin) == wanted).then_some(repo)
        })
}

/// Where this host keeps its own copy of a lead's project it has no checkout
/// of.
fn clone_dir(core: &Core, host: &str, code: &Code) -> PathBuf {
    use sha2::Digest;
    let key = match &code.remote {
        Some(remote) => normalized(remote),
        None => format!("{host}:{}", code.project),
    };
    let digest = hex::encode(sha2::Sha256::digest(key.as_bytes()));
    let base = core
        .listening
        .get()
        .and_then(|listening| listening.config.data_dir.clone())
        .unwrap_or_else(std::env::temp_dir)
        .join(CLONES);
    base.join(format!("{}-{}", slug(&code.project), &digest[..12]))
}

/// The commits a host with `repo` has, so a bundle need not carry them.
fn tips(repo: &Path) -> Vec<String> {
    git(
        repo,
        &[
            "for-each-ref",
            "--count=64",
            "--sort=-committerdate",
            "--format=%(objectname)",
            "refs/heads",
            "refs/remotes",
        ],
    )
    .map(|out| out.lines().map(str::to_owned).collect())
    .unwrap_or_default()
}

/// Where a worker's code was cut and the folder it works in, or the commits
/// a bundle should leave out.
pub(crate) enum Checked {
    Ready(Tree, PathBuf),
    Needs(Vec<String>),
}

fn relative(inside: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(inside);
    if path
        .components()
        .all(|part| matches!(part, Component::Normal(_)))
    {
        Ok(path)
    } else {
        Err("the lead's folder is not inside its repository".into())
    }
}

/// Cuts a checkout of the lead's code for the worker called `name`, from the
/// project here or a copy of its own, with `bundle` when the lead sent one.
pub(crate) fn checkout(
    core: &Core,
    host: &str,
    agent_id: &str,
    code: &Code,
    bundle: Option<&Path>,
    name: &str,
) -> Result<Checked, String> {
    if !is_commit_id(&code.commit) {
        return Err("the lead's commit is not a commit id".into());
    }
    let inside = relative(&code.inside)?;
    let incoming = format!("refs/sikemux/incoming/{}", uuid::Uuid::new_v4().simple());
    let repo = match code
        .remote
        .as_deref()
        .and_then(|remote| project_with(core, remote))
    {
        Some(repo) => repo,
        None => {
            let dir = clone_dir(core, host, code);
            if !dir.join(".git").exists() {
                let Some(_) = bundle else {
                    return Ok(Checked::Needs(Vec::new()));
                };
                std::fs::create_dir_all(&dir)
                    .map_err(|error| format!("could not make {}: {error}", dir.display()))?;
                git(&dir, &["init", "--quiet"])?;
                if let Some(remote) = &code.remote {
                    let _ = git(&dir, &["remote", "add", "origin", remote]);
                }
            }
            dir
        }
    };
    if !has_commit(&repo, &code.commit) {
        let Some(bundle) = bundle else {
            return Ok(Checked::Needs(tips(&repo)));
        };
        fetch(&repo, bundle, &incoming)?;
        if !has_commit(&repo, &code.commit) {
            let _ = git(&repo, &["update-ref", "-d", &incoming]);
            return Err("the lead's bundle did not hold its commit".into());
        }
    }
    let folder = worktrees_folder(&repo);
    let base = slug(name);
    let (branch, path) = (1..=50)
        .map(|index| {
            let slug = if index == 1 {
                base.clone()
            } else {
                format!("{base}-{index}")
            };
            (format!("sikemux/{slug}"), folder.join(&slug))
        })
        .find(|(branch, path)| !path.exists() && !branch_exists(&repo, branch))
        .ok_or("every worktree name for this worker is taken")?;
    std::fs::create_dir_all(&folder)
        .map_err(|error| format!("could not make {}: {error}", folder.display()))?;
    let target = path.to_string_lossy().into_owned();
    let added = git_in(
        &repo,
        &[
            "worktree",
            "add",
            "--no-track",
            "-b",
            &branch,
            &target,
            &code.commit,
        ],
        SLOW,
    );
    let _ = git(&repo, &["update-ref", "-d", &incoming]);
    added?;
    if let Some(patch) = &code.patch {
        if let Err(error) = apply(&path, patch) {
            remove(&repo, &path, &branch);
            return Err(error);
        }
    }
    Ok(Checked::Ready(
        Tree {
            agent_id: agent_id.to_owned(),
            repo,
            path: path.clone(),
            branch,
            base: code.commit.clone(),
            sent: None,
        },
        path.join(inside),
    ))
}

/// Applies the lead's uncommitted changes and commits them on their own, so
/// the worker's work reads apart from the lead's.
fn apply(tree: &Path, patch: &str) -> Result<(), String> {
    let file = Scratch::new("patch");
    std::fs::write(file.path(), patch)
        .map_err(|error| format!("could not write the lead's changes: {error}"))?;
    git(
        tree,
        &[
            "apply",
            "--binary",
            "--whitespace=nowarn",
            &file.path().to_string_lossy(),
        ],
    )
    .map_err(|error| format!("the lead's uncommitted changes do not apply here: {error}"))?;
    git(tree, &["add", "--all"])?;
    let mut args = Vec::new();
    if git(tree, &["config", "user.email"]).is_err() {
        args.extend([
            "-c",
            "user.name=Sikemux",
            "-c",
            "user.email=sikemux@localhost",
        ]);
    }
    args.extend([
        "commit",
        "--quiet",
        "--no-verify",
        "-m",
        "The lead's uncommitted changes, as they were when it started this worker",
    ]);
    git_in(tree, &args, SLOW).map(drop)
}

/// Takes away a worker's checkout and its branch.
pub(crate) fn remove(repo: &Path, tree: &Path, branch: &str) {
    let target = tree.to_string_lossy().into_owned();
    let _ = git(repo, &["worktree", "remove", "--force", &target]);
    let _ = git(repo, &["branch", "-D", branch]);
}

/// A bundle at `into` of the worker's commits since the lead's commit, and
/// the commit it ends at and how many it holds, unless nothing is new since
/// the last went back.
pub(crate) fn commits(tree: &Tree, into: &Path) -> Result<Option<(String, usize)>, String> {
    let head = git(&tree.path, &["rev-parse", "HEAD"])?;
    if head == tree.base || tree.sent.as_deref() == Some(head.as_str()) {
        return Ok(None);
    }
    let count: usize = git(
        &tree.path,
        &["rev-list", "--count", &format!("{}..{head}", tree.base)],
    )?
    .parse()
    .unwrap_or(0);
    if count == 0 {
        return Ok(None);
    }
    let bundle = bundle(&tree.path, &head, std::slice::from_ref(&tree.base))?;
    std::fs::rename(bundle.path(), into)
        .or_else(|_| std::fs::copy(bundle.path(), into).map(drop))
        .map_err(|error| format!("could not keep the bundle: {error}"))?;
    Ok(Some((head, count)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Landing {
    agent_id: String,
    count: usize,
}

/// A worker on `host` sent its commits: they go onto a branch named for it
/// in its lead's checkout here, which its next report names.
pub(crate) async fn land(
    core: &Arc<Core>,
    host: &str,
    params: Value,
    body: Option<Received>,
) -> Result<Value, String> {
    let landing: Landing = serde_json::from_value(params).map_err(|error| error.to_string())?;
    let worker = core
        .hosts
        .team
        .get(&landing.agent_id)
        .filter(|worker| worker.host == host && worker.role == Role::Worker)
        .ok_or("that worker is not one this host started")?;
    let repo = worker
        .repo
        .clone()
        .ok_or("this host does not know where that worker's lead works")?;
    let body = body.ok_or("the commits came without their bundle")?;
    let branch = format!(
        "sikemux/{}/{}",
        slug(&name_of(core, host)),
        slug(&worker.name)
    );
    let landed = {
        let branch = branch.clone();
        blocking(move || {
            git(&repo, &["check-ref-format", "--branch", &branch])?;
            fetch(&repo, body.path(), &format!("refs/heads/{branch}"))?;
            Ok(())
        })
        .await
    };
    landed.map_err(|error| error.to_string())?;
    let count = landing.count;
    core.hosts.team.update(&landing.agent_id, |worker| {
        worker.landed = Some((branch.clone(), count));
    });
    Ok(json!({ "branch": branch }))
}
