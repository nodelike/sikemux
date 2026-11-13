//! The code a worker on another host starts from, and the commits it sends
//! back. The lead's host sends its commit and, as a patch, what it has not
//! committed. The worker's host cuts a worktree of the project when it has it,
//! found by its `origin`, fetching the commit from a git bundle when it lacks
//! it, or keeps a copy of its own in its data folder. The worker's commits come
//! back as a bundle onto a branch named for it, which the lead merges or not.

pub(crate) use crate::server::board::git::identity;
use crate::server::parts::Parts;
use crate::server::workspace_state::WorkspaceParts;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::super::connection::blocking;
use super::super::worktree::{self, inside, slug};
use super::super::{new_project, Core};
use super::calls::{self as asks, params_of, HostAsk};
use super::link::{Received, Refusal};
use super::name_of;
use super::team::{Role, Tree};
use crate::git::Git;

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
    /// The remote it is known by, which a host that has the project finds it by.
    pub remote: Option<String>,
    /// What the repository is known by on every host (ADR 0023), which a host finds its own
    /// checkout of it by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    /// Its `origin`, which a new checkout clones from and pushes to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    pub commit: String,
    pub branch: Option<String>,
    /// Where the lead works inside the repository.
    #[serde(default)]
    pub inside: String,
    /// What the lead has not committed, as a binary patch on `commit`.
    pub patch: Option<String>,
}

impl Code {
    /// Where a new checkout of it fetches from: its `origin`, else the remote it is known by.
    pub(crate) fn fetch_url(&self) -> Option<&String> {
        self.origin.as_ref().or(self.remote.as_ref())
    }
}

/// The lead's repository and what a worker elsewhere needs of it.
pub(crate) struct Packed {
    pub repo: PathBuf,
    pub code: Code,
    /// How many changed files stay behind, when what is not committed does not go.
    pub left_behind: usize,
}

/// A file made for the link, removed once it is dropped.
pub(crate) struct Scratch(PathBuf);

impl Scratch {
    pub(crate) fn new(name: &str) -> Self {
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
    Git::quick(repo)
        .trimmed(&["cat-file", "-e", &format!("{commit}^{{commit}}")])
        .is_ok()
}

/// What another host needs to open the project at `cwd`, with what is not committed only when
/// `uncommitted` asks for it.
pub(crate) fn package_for(cwd: &Path, uncommitted: bool) -> Result<Packed, String> {
    let repo = PathBuf::from(
        Git::quick(cwd)
            .trimmed(&["rev-parse", "--path-format=absolute", "--show-toplevel"])
            .map_err(|_| {
                "your folder is not in a git repository, so its code cannot go to another host"
                    .to_owned()
            })?,
    );
    let commit = Git::quick(&repo).trimmed(&["rev-parse", "HEAD"]).map_err(|_| {
        "your repository has no commit yet, so a worker on another host has nothing to start from"
            .to_owned()
    })?;
    let project = repo
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".to_owned());
    let code = Code {
        project,
        remote: crate::git::remote::identity_url(&repo.to_string_lossy()),
        identity: crate::git::remote::project_identity(&repo.to_string_lossy()),
        origin: Git::quick(&repo)
            .trimmed(&["remote", "get-url", "origin"])
            .ok()
            .filter(|url| !url.is_empty()),
        branch: Git::quick(&repo)
            .trimmed(&["symbolic-ref", "--quiet", "--short", "HEAD"])
            .ok(),
        inside: inside(cwd, &repo).to_string_lossy().into_owned(),
        patch: match uncommitted {
            true => self::uncommitted(&repo)?,
            false => None,
        },
        commit,
    };
    let left_behind = match uncommitted {
        true => 0,
        false => Git::slow(&repo)
            .trimmed(&["status", "--porcelain"])
            .map_or(0, |status| status.lines().count()),
    };
    Ok(Packed {
        repo,
        code,
        left_behind,
    })
}

/// Everything not committed, new files too, as a patch on `HEAD`, worked out
/// in an index of its own so the lead's staging stays as it is.
fn uncommitted(repo: &Path) -> Result<Option<String>, String> {
    let index = Git::quick(repo).trimmed(&[
        "rev-parse",
        "--path-format=absolute",
        "--git-path",
        "index",
    ])?;
    let scratch = Scratch::new("index");
    if Path::new(&index).exists() {
        std::fs::copy(&index, scratch.path())
            .map_err(|error| format!("could not read the repository's index: {error}"))?;
    }
    Git::slow(repo)
        .index(scratch.path())
        .trimmed(&["add", "--all"])?;
    let patch = Scratch::new("patch");
    let output = format!("--output={}", patch.path().display());
    Git::slow(repo).index(scratch.path()).trimmed(&[
        "diff",
        "--cached",
        "--binary",
        "--no-color",
        &output,
        "HEAD",
    ])?;
    let text = std::fs::read_to_string(patch.path()).unwrap_or_default();
    if text.len() > MAX_PATCH_BYTES {
        return Err(format!(
            "your uncommitted changes come to {} MB, more than goes to another host; commit them first, or leave them out",
            text.len() / (1024 * 1024)
        ));
    }
    Ok((!text.trim().is_empty()).then_some(text))
}

/// A bundle of `commit` without what a host holding `tips` already has.
pub(crate) fn bundle(repo: &Path, commit: &str, tips: &[String]) -> Result<Scratch, String> {
    let reference = format!("refs/sikemux/outgoing/{}", uuid::Uuid::new_v4().simple());
    Git::quick(repo).trimmed(&["update-ref", &reference, commit])?;
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
    let made = watched(Git::at(repo), &args, scratch.path());
    let _ = Git::quick(repo).trimmed(&["update-ref", "-d", &reference]);
    made?;
    Ok(scratch)
}

/// The one branch tip a bundle carries.
fn bundled_ref(repo: &Path, bundle: &Path) -> Result<String, String> {
    let bundle = bundle.to_string_lossy().into_owned();
    let heads = Git::quick(repo).trimmed(&["bundle", "list-heads", &bundle])?;
    heads
        .lines()
        .find_map(|line| line.split_whitespace().nth(1))
        .filter(|reference| reference.starts_with("refs/"))
        .map(str::to_owned)
        .ok_or_else(|| "the bundle holds no branch".to_owned())
}

/// `wanted`, or `wanted-2` and on when a branch of that name is there already,
/// so a worker's commits never replace another's that nobody merged.
fn free_branch(repo: &Path, wanted: &str) -> Result<String, String> {
    (1..100)
        .map(|n| match n {
            1 => wanted.to_owned(),
            n => format!("{wanted}-{n}"),
        })
        .find(|branch| {
            Git::quick(repo)
                .trimmed(&[
                    "show-ref",
                    "--verify",
                    "--quiet",
                    &format!("refs/heads/{branch}"),
                ])
                .is_err()
        })
        .ok_or_else(|| {
            format!(
                "{} already has too many branches named {wanted}",
                repo.display()
            )
        })
}

/// Whether `branch` is gone, or still at the commit this worker's commits
/// last came back at, so nobody else's commits are on it.
fn still_own(repo: &Path, branch: &str, last_head: Option<&str>) -> bool {
    match Git::quick(repo).trimmed(&[
        "rev-parse",
        "--verify",
        "--quiet",
        &format!("refs/heads/{branch}"),
    ]) {
        Err(_) => true,
        Ok(tip) => last_head == Some(tip.as_str()),
    }
}

fn fetch(repo: &Path, bundle: &Path, into: &str) -> Result<(), String> {
    Git::slow(repo)
        .trimmed(&["bundle", "verify", "--quiet", &bundle.to_string_lossy()])
        .map_err(|error| format!("the bundle does not fit this repository: {error}"))?;
    let reference = bundled_ref(repo, bundle)?;
    let refspec = format!("+{reference}:{into}");
    let args = [
        "fetch",
        "--quiet",
        "--no-write-fetch-head",
        &bundle.to_string_lossy(),
        &refspec,
    ];
    watched(Git::at(repo), &args, &repo.join(".git")).map(drop)
}

/// How long git may write nothing before a clone or fetch is judged stalled.
const STALLED: Duration = Duration::from_secs(5 * 60);
/// The longest a clone or fetch that keeps writing may run.
const LONGEST: Duration = Duration::from_secs(6 * 60 * 60);

/// How many bytes the files under `dir` hold, or the file `dir` and the lock git writes it
/// through.
fn written(dir: &Path) -> u64 {
    if !dir.is_dir() {
        let mut lock = dir.as_os_str().to_owned();
        lock.push(".lock");
        return [dir, Path::new(&lock)]
            .iter()
            .filter_map(|file| std::fs::metadata(file).ok())
            .map(|meta| meta.len())
            .sum();
    }
    let mut total = 0;
    let mut folders = vec![dir.to_owned()];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => folders.push(entry.path()),
                Ok(_) => total += entry.metadata().map_or(0, |meta| meta.len()),
                Err(_) => {}
            }
        }
    }
    total
}

/// Runs `git args` for as long as it keeps writing into `dir`, ending it once nothing has
/// been written there for [`STALLED`].
fn watched(git: Git, args: &[&str], dir: &Path) -> Result<String, String> {
    let cancel = sikemux_process::ProcessCancellation::new();
    let (finished, wait) = std::sync::mpsc::channel::<()>();
    let stalled = std::thread::scope(|scope| {
        let cancel = &cancel;
        let watcher = scope.spawn(move || {
            let mut last = (written(dir), std::time::Instant::now());
            while let Err(std::sync::mpsc::RecvTimeoutError::Timeout) =
                wait.recv_timeout(Duration::from_secs(10))
            {
                let now = written(dir);
                if now != last.0 {
                    last = (now, std::time::Instant::now());
                } else if last.1.elapsed() > STALLED {
                    cancel.cancel();
                    return true;
                }
            }
            false
        });
        let ran = git.timeout(LONGEST).cancel_on(cancel).trimmed(args);
        drop(finished);
        (watcher.join().unwrap_or(false), ran)
    });
    match stalled {
        (true, _) => Err(format!(
            "git wrote nothing for {} minutes, so it was stopped",
            STALLED.as_secs() / 60
        )),
        (false, ran) => ran,
    }
}

/// The checkouts of this host's projects of the repository `code` comes from: the same
/// repository identity, which a repository with no remote has too, else, from a host that sends
/// none, one of the same remote.
fn projects_with(core: &Core, code: &Code) -> Vec<PathBuf> {
    let mut repos: Vec<PathBuf> = (core.workspaces().project_paths().into_iter())
        .filter_map(|project| {
            let repo = Git::quick(&project)
                .trimmed(&["rev-parse", "--path-format=absolute", "--show-toplevel"])
                .ok()?;
            let same = match (&code.identity, &code.remote) {
                (Some(identity), _) => {
                    crate::git::remote::project_identity(&repo).as_ref() == Some(identity)
                }
                (None, Some(remote)) => crate::git::remote::has_remote(&repo, remote),
                (None, None) => false,
            };
            same.then(|| PathBuf::from(repo))
        })
        .collect();
    repos.sort();
    repos.dedup();
    repos
}

/// The checkout here a worker's worktree is cut from: the only one of the repository, else the
/// only one that already has the lead's commit. With several and nothing to choose by, none,
/// so the worker gets this host's own copy rather than whichever checkout sorts first.
fn worker_repo(core: &Core, code: &Code) -> Option<PathBuf> {
    let repos = projects_with(core, code);
    if let [only] = repos.as_slice() {
        return Some(only.clone());
    }
    let mut holding = repos
        .into_iter()
        .filter(|repo| has_commit(repo, &code.commit));
    match (holding.next(), holding.next()) {
        (Some(repo), None) => Some(repo),
        _ => None,
    }
}

/// Where this host keeps its own copy of a lead's project it has no checkout
/// of.
fn clone_dir(core: &Core, host: &str, code: &Code) -> PathBuf {
    use sha2::Digest;
    let key = match &code.remote {
        Some(remote) => crate::git::remote::normalized_remote(remote),
        None => format!("{host}:{}", code.project),
    };
    let digest = hex::encode(sha2::Sha256::digest(key.as_bytes()));
    let base = core
        .config()
        .data_dir
        .clone()
        .unwrap_or_else(std::env::temp_dir)
        .join(CLONES);
    base.join(format!("{}-{}", slug(&code.project), &digest[..12]))
}

/// The commits a host with `repo` has, so a bundle need not carry them.
fn tips(repo: &Path) -> Vec<String> {
    Git::quick(repo)
        .trimmed(&[
            "for-each-ref",
            "--count=64",
            "--sort=-committerdate",
            "--format=%(objectname)",
            "refs/heads",
            "refs/remotes",
        ])
        .map(|out| out.lines().map(str::to_owned).collect())
        .unwrap_or_default()
}

/// Where a project the person opened on this host from another one lives, or the
/// folder claimed for it and the commits a bundle should leave out.
pub(crate) enum Opened {
    /// The project's checkout, and the folder the work goes on in: the checkout itself, or a
    /// worktree of it at the commit sent.
    Ready {
        repo: PathBuf,
        work: PathBuf,
    },
    Needs {
        into: PathBuf,
        tips: Vec<String>,
    },
    /// The checkout here is not where the sent work stands, so laying it there would mix the
    /// two; the work can go into a worktree at the commit sent instead.
    Differs(String),
}

/// The folder name a project takes here: its repository's, else its own.
fn folder_name(code: &Code) -> String {
    let from_remote = code.remote.as_deref().and_then(new_project::repo_name);
    from_remote.unwrap_or_else(|| new_project::folder_name(&code.project))
}

/// The folders this host began for projects from other hosts and has not finished, kept in its
/// own data folder: only these may be filled or removed, never a folder that merely looks like
/// one.
const CLAIMS_FILE: &str = "host-claims.json";

#[derive(Clone, Serialize, Deserialize)]
struct Claim {
    path: PathBuf,
    /// What it was begun for: the commit, the repository and whether it is a new copy.
    key: String,
}

fn claims_file(core: &Core) -> Option<PathBuf> {
    core.config()
        .data_dir
        .as_ref()
        .map(|dir| dir.join(CLAIMS_FILE))
}

fn claims_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn read_claims(core: &Core) -> Vec<Claim> {
    (claims_file(core).and_then(|file| std::fs::read(file).ok()))
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn write_claims(core: &Core, claims: &[Claim]) -> Result<(), String> {
    let file = claims_file(core).ok_or("this host has no data folder to keep its copies in")?;
    let bytes = serde_json::to_vec(claims).map_err(|error| error.to_string())?;
    sikemux_process::files::write_atomic(&file, &bytes, false).map_err(|error| error.to_string())
}

fn claim_key(code: &Code, copy: bool) -> String {
    let repository = code.remote.as_deref().unwrap_or(&code.project);
    format!("{}:{repository}:{copy}", code.commit)
}

/// Whether `dir` is a folder this host began for a project from another host and has not
/// finished: recorded as such here, and not one of this host's projects.
pub(crate) fn claimed(core: &Core, dir: &Path) -> bool {
    let recorded = {
        let _held = claims_lock();
        read_claims(core).iter().any(|claim| claim.path == dir)
    };
    recorded && !is_project_repo(core, dir)
}

fn unclaim(core: &Core, dir: &Path) {
    let _held = claims_lock();
    let mut claims = read_claims(core);
    let before = claims.len();
    claims.retain(|claim| claim.path != dir);
    if claims.len() != before {
        let _ = write_claims(core, &claims);
    }
}

/// Removes `dir` when it is a folder this host began and did not finish.
pub(crate) fn abandon(core: &Core, dir: &Path) {
    if claimed(core, dir) {
        let _ = std::fs::remove_dir_all(dir);
    }
    unclaim(core, dir);
}

/// A new folder for a project opened from another host: in the person's projects folder, else
/// in this host's data folder, under a new name when its own is taken.
fn opened_dir(core: &Core, code: &Code) -> Result<PathBuf, String> {
    let parent = new_project::projects_folder(core)
        .or_else(|| core.config().data_dir.as_ref().map(|dir| dir.join(CLONES)))
        .ok_or("this host has no folder outside a git repository to clone into")?;
    new_project::claim_folder(&parent, &folder_name(code))
}

/// Claims a folder for `code`: the one an earlier ask for the same opening began and did not
/// finish, else a new one, cloned from its remote with this host's own sign-in when no bundle
/// came, else an empty repository a bundle fills.
fn claim(core: &Core, code: &Code, bundle: Option<&Path>, copy: bool) -> Result<PathBuf, String> {
    let key = claim_key(code, copy);
    let begun = {
        let _held = claims_lock();
        read_claims(core)
            .into_iter()
            .find(|claim| claim.key == key && claim.path.join(".git").exists())
    };
    if let Some(begun) = begun.filter(|begun| !is_project_repo(core, &begun.path)) {
        return Ok(begun.path);
    }
    let dir = opened_dir(core, code)?;
    {
        let _held = claims_lock();
        let mut claims = read_claims(core);
        claims.push(Claim {
            path: dir.clone(),
            key,
        });
        write_claims(core, &claims)?;
    }
    if let (Some(remote), None) = (code.fetch_url(), bundle) {
        let parent = dir
            .parent()
            .ok_or("the folder to clone into has no parent")?;
        let target = dir.to_string_lossy().into_owned();
        let ssh = sikemux_process::user_environment::var("GIT_SSH_COMMAND")
            .unwrap_or_else(|| "ssh -o BatchMode=yes -o ConnectTimeout=20".into());
        let clone = Git::at(parent).env("GIT_SSH_COMMAND", &ssh);
        if watched(clone, &["clone", "--quiet", "--", remote, &target], &dir).is_ok() {
            return Ok(dir);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
    let begun = std::fs::create_dir_all(&dir)
        .map_err(|error| format!("could not make {}: {error}", dir.display()))
        .and_then(|()| Git::quick(&dir).trimmed(&["init", "--quiet"]));
    if let Err(error) = begun {
        let _ = std::fs::remove_dir_all(&dir);
        unclaim(core, &dir);
        return Err(error);
    }
    if let Some(remote) = code.fetch_url() {
        let _ = Git::quick(&dir).trimmed(&["remote", "add", "origin", remote]);
    }
    Ok(dir)
}

/// Fetches `commit` into `repo` from `bundle`, keeping no branch for it.
fn take_commit(repo: &Path, bundle: &Path, commit: &str) -> Result<(), String> {
    let incoming = format!("refs/sikemux/incoming/{}", uuid::Uuid::new_v4().simple());
    let fetched = fetch(repo, bundle, &incoming);
    let _ = Git::quick(repo).trimmed(&["update-ref", "-d", &incoming]);
    fetched?;
    match has_commit(repo, commit) {
        true => Ok(()),
        false => Err("the bundle did not hold the project's commit".into()),
    }
}

/// Brings the claimed checkout `dir` to the commit sent and its branch, from `bundle` when it
/// lacks the commit, after which it is no longer claimed.
fn fill(core: &Core, dir: &Path, code: &Code, bundle: Option<&Path>) -> Result<Opened, String> {
    if !has_commit(dir, &code.commit) {
        let Some(bundle) = bundle else {
            return Ok(Opened::Needs {
                into: dir.to_owned(),
                tips: tips(dir),
            });
        };
        take_commit(dir, bundle, &code.commit)?;
    }
    let checkout = match &code.branch {
        Some(branch) => vec!["checkout", "--quiet", "-B", branch, &code.commit],
        None => vec!["checkout", "--quiet", "--detach", &code.commit],
    };
    watched(Git::at(dir), &checkout, dir)?;
    unclaim(core, dir);
    Ok(Opened::Ready {
        repo: dir.to_owned(),
        work: dir.to_owned(),
    })
}

/// Why the checkout `repo` is not where `code` stands: another commit or branch checked out,
/// or changes of its own under the uncommitted changes sent.
fn differs(repo: &Path, code: &Code) -> Option<String> {
    let git = || Git::quick(repo);
    let head = git().trimmed(&["rev-parse", "HEAD"]).unwrap_or_default();
    let branch = git()
        .trimmed(&["symbolic-ref", "--quiet", "--short", "HEAD"])
        .ok();
    let word = |branch: &Option<String>| branch.clone().unwrap_or_else(|| "no branch".into());
    if branch != code.branch {
        return Some(format!(
            "its checkout there is on {}, not {}",
            word(&branch),
            word(&code.branch)
        ));
    }
    if head != code.commit {
        return Some(format!(
            "its checkout there is at another commit of {}",
            word(&branch)
        ));
    }
    let dirty = !git()
        .trimmed(&["status", "--porcelain"])
        .unwrap_or_default()
        .is_empty();
    (dirty && code.patch.is_some())
        .then(|| "its checkout there has uncommitted changes of its own".to_owned())
}

/// The repository at the top of `path`.
fn toplevel(path: &Path) -> Option<PathBuf> {
    Git::quick(path)
        .trimmed(&["rev-parse", "--path-format=absolute", "--show-toplevel"])
        .ok()
        .map(PathBuf::from)
}

/// Which of this host's checkouts `repos` of the sent repository the work goes into: the one
/// already at the sent commit and branch, else the project `there` the person has in front
/// here, else the only one. With several and nothing to choose by, it refuses and names them.
fn chosen_clone(
    core: &Core,
    code: &Code,
    repos: Vec<PathBuf>,
    there: Option<&str>,
) -> Result<Option<PathBuf>, String> {
    let standing =
        |repo: &&PathBuf| has_commit(repo, &code.commit) && differs(repo, code).is_none();
    if let Some(repo) = repos.iter().find(standing) {
        return Ok(Some(repo.clone()));
    }
    let chosen = (there.and_then(|id| core.workspaces().project_info(id)))
        .and_then(|project| toplevel(&project.path))
        .filter(|repo| repos.contains(repo));
    match (chosen, repos.as_slice()) {
        (Some(repo), _) => Ok(Some(repo)),
        (None, []) => Ok(None),
        (None, [only]) => Ok(Some(only.clone())),
        (None, many) => Err(format!(
            "this host has {} checkouts of it ({}) and none is at the sent commit; put the one to use in front in the copy switch, then send again",
            many.len(),
            (many.iter())
                .map(|repo| repo.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// Whether `dir` is the repository of one of this host's projects.
fn is_project_repo(core: &Core, dir: &Path) -> bool {
    core.workspaces().project_paths().iter().any(|project| {
        Git::quick(project)
            .trimmed(&["rev-parse", "--path-format=absolute", "--show-toplevel"])
            .is_ok_and(|top| Path::new(&top) == dir)
    })
}

/// Opens the person's project from another host here: a checkout of the same `origin` when it
/// stands where the sent work does, or a worktree of it at the commit sent when `worktree`
/// asks, unless `copy` asks for a new one, else a new folder cloned from the remote with this
/// host's own sign-in, else filled from `bundle`, at the commit sent and on its branch. A
/// second round names the folder the first claimed in `into`; a folder claimed here that
/// fails is removed. Only committed work comes over.
pub(crate) fn open_project(
    core: &Core,
    code: &Code,
    bundle: Option<&Path>,
    copy: bool,
    into: Option<&Path>,
    worktree: bool,
    there: Option<&str>,
) -> Result<Opened, String> {
    if !is_commit_id(&code.commit) {
        return Err("the project's commit is not a commit id".into());
    }
    let existing = match into {
        Some(dir) if claimed(core, dir) => None,
        Some(dir) if is_project_repo(core, dir) => Some(dir.to_owned()),
        Some(_) => return Err("the folder begun for this project is gone".into()),
        None if copy => None,
        None => {
            let repos = projects_with(core, code);
            chosen_clone(core, code, repos, there)?
        }
    };
    if let Some(repo) = existing {
        if !has_commit(&repo, &code.commit) {
            let Some(bundle) = bundle else {
                return Ok(Opened::Needs {
                    tips: tips(&repo),
                    into: repo,
                });
            };
            take_commit(&repo, bundle, &code.commit)?;
        }
        return match worktree {
            true => {
                let branch = format!("sent/{}", code.branch.as_deref().unwrap_or("work"));
                let (_, work) = worktree::cut(&repo, &branch, &code.commit)?;
                Ok(Opened::Ready { repo, work })
            }
            false => Ok(match differs(&repo, code) {
                Some(why) => Opened::Differs(why),
                None => Opened::Ready {
                    work: repo.clone(),
                    repo,
                },
            }),
        };
    }
    let dir = match into {
        Some(dir) => dir.to_owned(),
        None => claim(core, code, bundle, copy)?,
    };
    let filled = fill(core, &dir, code, bundle);
    if filled.is_err() {
        abandon(core, &dir);
    }
    filled
}

/// Where a worker's code was cut and the folder it works in, or the commits
/// a bundle should leave out.
pub(crate) enum Checked {
    Ready(Tree, PathBuf),
    Needs(Vec<String>),
}

pub(crate) fn relative(inside: &str) -> Result<PathBuf, String> {
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

/// Cuts a checkout of the lead's code on the branch `branch`, from the project
/// here or a copy of its own, with `bundle` when the lead sent one.
pub(crate) fn checkout(
    core: &Core,
    host: &str,
    agent_id: &str,
    code: &Code,
    bundle: Option<&Path>,
    branch: &str,
) -> Result<Checked, String> {
    if !is_commit_id(&code.commit) {
        return Err("the lead's commit is not a commit id".into());
    }
    let inside = relative(&code.inside)?;
    let incoming = format!("refs/sikemux/incoming/{}", uuid::Uuid::new_v4().simple());
    let repo = match worker_repo(core, code) {
        Some(repo) => repo,
        None => {
            let dir = clone_dir(core, host, code);
            if !dir.join(".git").exists() {
                let Some(_) = bundle else {
                    return Ok(Checked::Needs(Vec::new()));
                };
                std::fs::create_dir_all(&dir)
                    .map_err(|error| format!("could not make {}: {error}", dir.display()))?;
                Git::quick(&dir).trimmed(&["init", "--quiet"])?;
                if let Some(remote) = code.fetch_url() {
                    let _ = Git::quick(&dir).trimmed(&["remote", "add", "origin", remote]);
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
            let _ = Git::quick(&repo).trimmed(&["update-ref", "-d", &incoming]);
            return Err("the lead's bundle did not hold its commit".into());
        }
    }
    let added = worktree::cut(&repo, branch, &code.commit);
    let _ = Git::quick(&repo).trimmed(&["update-ref", "-d", &incoming]);
    let (branch, path) = added?;
    if let Some(patch) = &code.patch {
        if let Err(error) = apply(&path, patch) {
            worktree::remove(&repo, &path, &branch);
            return Err(error);
        }
    }
    worktree::seed_in_background(&repo, &path);
    Ok(Checked::Ready(
        Tree {
            agent_id: agent_id.to_owned(),
            repo,
            path: path.clone(),
            branch,
            base: code.commit.clone(),
            sent: None,
            auto: false,
        },
        path.join(inside),
    ))
}

/// Lays `patch`, another host's uncommitted changes, over the checkout at `tree`, leaving
/// them uncommitted. Nothing changes when any of it does not apply.
pub(crate) fn lay_over(tree: &Path, patch: &str) -> Result<(), String> {
    let file = Scratch::new("patch");
    std::fs::write(file.path(), patch)
        .map_err(|error| format!("could not write the changes: {error}"))?;
    Git::quick(tree)
        .trimmed(&[
            "apply",
            "--binary",
            "--whitespace=nowarn",
            &file.path().to_string_lossy(),
        ])
        .map(drop)
        .map_err(|error| format!("the uncommitted changes do not apply here: {error}"))
}

/// Applies the uncommitted changes of the copy a new copy was made from and
/// commits them on their own, as a worker gets its lead's.
pub(crate) fn commit_over(tree: &Path, patch: &str) -> Result<(), String> {
    lay_over(tree, patch)?;
    commit_everything(
        tree,
        "The uncommitted changes of the copy this one was made from, as they were then",
    )
}

/// Applies the lead's uncommitted changes and commits them on their own, so
/// the worker's work reads apart from the lead's.
fn apply(tree: &Path, patch: &str) -> Result<(), String> {
    lay_over(tree, patch)?;
    commit_everything(
        tree,
        "The lead's uncommitted changes, as they were when it started this worker",
    )
}

fn commit_everything(tree: &Path, message: &str) -> Result<(), String> {
    Git::quick(tree).trimmed(&["add", "--all"])?;
    let mut args = identity(tree);
    args.extend(["commit", "--quiet", "--no-verify", "-m", message]);
    Git::slow(tree).trimmed(&args).map(drop)
}

/// A bundle at `into` of the worker's commits since the lead's commit, and
/// the commit it ends at and how many it holds, unless nothing is new since
/// the last went back.
pub(crate) fn commits(tree: &Tree, into: &Path) -> Result<Option<(String, usize)>, String> {
    if tree.auto
        && !Git::quick(&tree.path)
            .trimmed(&["status", "--porcelain"])?
            .is_empty()
    {
        commit_everything(
            &tree.path,
            "What its agent left uncommitted when its turn ended",
        )?;
    }
    let head = Git::quick(&tree.path).trimmed(&["rev-parse", "HEAD"])?;
    if head == tree.base || tree.sent.as_deref() == Some(head.as_str()) {
        return Ok(None);
    }
    let count: usize = Git::quick(&tree.path)
        .trimmed(&["rev-list", "--count", &format!("{}..{head}", tree.base)])?
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

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ArrivingCommits {
    pub(super) agent_id: String,
    #[serde(default)]
    pub(super) head: String,
    pub(super) count: usize,
}

/// A worker on `host` sent its commits: they go onto a branch named for it
/// in its lead's checkout here, which its next report names. Commits that cannot land for good
/// are told to its lead; the worker's host keeps them to send again.
pub(super) async fn land(
    core: &Arc<Core>,
    host: &str,
    params: ArrivingCommits,
    body: Option<Received>,
) -> Result<Value, Refusal> {
    let landing = params;
    let worker = core
        .hosts()
        .team
        .get(&landing.agent_id)
        .filter(|worker| worker.host == host && worker.role == Role::Worker)
        .ok_or("that worker is not one this host started")?;
    let refused = |why: String| {
        if let Some(lead) = worker.parent.clone() {
            super::route::notice(core, lead, format!("The commits {} on {} sent back did not land in your checkout: {why}. They stay in its checkout there.", worker.name, name_of(core, host)));
        }
        Refusal::from(why)
    };
    let repo = worker
        .repo
        .clone()
        .ok_or_else(|| refused("this host does not know where that worker's lead works".into()))?;
    if !repo.is_dir() {
        return Err(refused(format!("{} is no longer there", repo.display())));
    }
    if worker.landed_head.as_deref() == Some(landing.head.as_str()) {
        return Ok(json!({ "branch": worker.branch }));
    }
    let body = match body {
        Some(body) => body,
        None => {
            let wanted = params_of::<asks::PullCommits>(&ArrivingCommits {
                agent_id: landing.agent_id.clone(),
                head: landing.head.clone(),
                count: landing.count,
            });
            super::pull(core, host, asks::PullCommits::METHOD, wanted)
                .await
                .map_err(|failed| match failed.passing() {
                    true => Refusal::retry(failed.to_string()),
                    false => Refusal::from(failed.to_string()),
                })?
        }
    };
    let (own, last_head) = (worker.branch, worker.landed_head);
    let wanted = format!(
        "sikemux/{}/{}",
        slug(&name_of(core, host)),
        slug(&worker.name)
    );
    let landed = blocking(move || {
        Git::quick(&repo).trimmed(&["check-ref-format", "--branch", &wanted])?;
        let branch = match own {
            Some(own) if still_own(&repo, &own, last_head.as_deref()) => own,
            _ => free_branch(&repo, &wanted)?,
        };
        fetch(&repo, body.path(), &format!("refs/heads/{branch}"))?;
        Ok(branch)
    })
    .await;
    let branch = landed.map_err(|error| Refusal::retry(error.to_string()))?;
    let count = landing.count;
    core.hosts().team.update(&landing.agent_id, |worker| {
        worker.landed = Some((branch.clone(), count));
        worker.landed_head = Some(landing.head.clone());
        worker.branch = Some(branch.clone());
    });
    Ok(json!({ "branch": branch }))
}
