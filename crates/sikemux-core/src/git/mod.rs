pub mod ai;
pub(crate) mod ai_provider;
pub mod blame;
pub mod branches;
pub mod changes;
pub mod commit;
pub mod diff;
pub mod log;
pub mod remote;
pub mod revisions;
pub mod stash;
pub mod status;
pub mod watch;
pub mod worktree;

use std::io::{self, Read};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use git2::Repository;
use tokio::task::spawn_blocking;

use crate::flights::shared;

/// Runs a synchronous closure off the async workers, so a libgit2 walk of a
/// big repo never stalls the core's other requests.
async fn run_blocking<T, E, F>(f: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, E> + Send + 'static,
    T: Send + 'static,
    E: std::fmt::Display + Send + 'static,
{
    spawn_blocking(f)
        .await
        .map_err(|e| format!("join: {e}"))
        .and_then(|r| r.map_err(|e| e.to_string()))
}

/// Cap on concurrent libgit2 tree walks (`git_status` / `git_log` /
/// `git_overview`). Each walk transiently opens a fistful of fds — index,
/// refs, packfiles, and the recursive untracked-dir scan. With many
/// projects fs-watched at once, a single `npm build` or a busy agent fans a
/// `git_changed` burst across every repo simultaneously; without a cap
/// that's N parallel walks all grabbing fds + CPU, a transient spike that
/// was a contributing factor to the EMFILE wall. 4 lets a few panes refresh
/// in parallel while bounding the peak.
const GIT_WALK_CONCURRENCY: usize = 4;

async fn git_walk_permit() -> Result<tokio::sync::SemaphorePermit<'static>, String> {
    static S: std::sync::OnceLock<tokio::sync::Semaphore> = std::sync::OnceLock::new();
    S.get_or_init(|| tokio::sync::Semaphore::new(GIT_WALK_CONCURRENCY))
        .acquire()
        .await
        .map_err(|e| e.to_string())
}

/// Walks being worked out, by what they answer.
type Flights<T> = crate::flights::Flights<String, Result<T, String>>;

/// A watched repo's walks are shared only with requests made at the same
/// generation, so none gets an answer from before a change it was told of.
fn flight_key(repo: &str, watched: Option<&(String, u64)>) -> String {
    match watched {
        Some((key, generation)) => format!("{generation}:{key}"),
        None => repo.to_owned(),
    }
}

fn open_repo(path: &str) -> Result<Repository, String> {
    Repository::discover(path).map_err(|e| e.message().to_string())
}

const GIT_COMMAND_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_COMMAND_OUTPUT_BYTES: usize = 32 * 1024 * 1024;
/// A git run on this computer alone, which ought to be over within a minute.
const QUICK: Duration = Duration::from_secs(60);
/// A git run that may take minutes, such as a fetch, a push or a big diff.
const SLOW: Duration = Duration::from_secs(5 * 60);
/// How much of a failure's output its error carries.
const TAIL_CHARS: usize = 3000;

fn read_bounded_pipe(mut pipe: impl Read, exceeded: Arc<AtomicBool>) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 16 * 1024];
    loop {
        let read = pipe.read(&mut chunk)?;
        if read == 0 {
            return Ok(bytes);
        }
        if bytes.len().saturating_add(read) > MAX_COMMAND_OUTPUT_BYTES {
            exceeded.store(true, Ordering::Release);
            return Err(io::Error::other("subprocess output exceeds 32 MiB limit"));
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
}

/// Every git process the core starts goes through here. A repository's own
/// config can name an fsmonitor program, which git would otherwise run on any
/// read of the index, and nobody is there to answer a prompt or an editor.
pub fn git_command(dir: impl AsRef<std::path::Path>) -> Command {
    let mut command = sikemux_process::user_environment::command("git");
    command
        .args(["-c", "core.fsmonitor=false"])
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_EDITOR", "true");
    command
}

/// One git run in a folder, bounded in time and output.
pub struct Git<'a> {
    command: Command,
    input: Option<&'a [u8]>,
    timeout: Duration,
    limit: usize,
}

impl<'a> Git<'a> {
    pub fn at(dir: impl AsRef<std::path::Path>) -> Self {
        Self {
            command: git_command(dir),
            input: None,
            timeout: GIT_COMMAND_TIMEOUT,
            limit: MAX_COMMAND_OUTPUT_BYTES,
        }
    }

    /// [`Self::at`] for a run on this computer alone, which ought to be over within a minute.
    pub fn quick(dir: impl AsRef<std::path::Path>) -> Self {
        Self::at(dir).timeout(QUICK)
    }

    /// [`Self::at`] for a run that may take minutes, such as a fetch, a push or a big diff.
    pub fn slow(dir: impl AsRef<std::path::Path>) -> Self {
        Self::at(dir).timeout(SLOW)
    }

    /// Sets `key` for this run only, as `git -c key=value` does.
    pub fn config(mut self, key: &str, value: &str) -> Self {
        self.command.arg("-c").arg(format!("{key}={value}"));
        self
    }

    pub fn env(mut self, key: &str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        self.command.env(key, value);
        self
    }

    /// Uses `index` in place of the checkout's own index.
    pub fn index(self, index: &std::path::Path) -> Self {
        self.env("GIT_INDEX_FILE", index)
    }

    pub fn input(mut self, input: &'a [u8]) -> Self {
        self.input = Some(input);
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn limit(mut self, bytes: usize) -> Self {
        self.limit = bytes;
        self
    }

    /// Runs `git args`, whatever it exits with.
    pub fn output(mut self, args: &[&str]) -> Result<Output, String> {
        self.command.args(args);
        sikemux_process::run(
            &mut self.command,
            self.input,
            self.timeout,
            self.limit,
            None,
        )
        .map_err(|error| format!("git did not run: {error}"))
    }

    /// What `git args` wrote, or what it said when it failed.
    pub fn stdout(self, args: &[&str]) -> Result<Vec<u8>, String> {
        let output = self.output(args)?;
        if output.status.success() {
            return Ok(output.stdout);
        }
        Err(format!(
            "git {} failed: {}",
            args.first().copied().unwrap_or_default(),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }

    /// [`Self::stdout`] as text.
    pub fn text(self, args: &[&str]) -> Result<String, String> {
        self.stdout(args)
            .map(|out| String::from_utf8_lossy(&out).into_owned())
    }

    /// What `git args` wrote, trimmed, or the end of all it wrote when it failed.
    pub fn trimmed(self, args: &[&str]) -> Result<String, String> {
        let output = self.output(args)?;
        if output.status.success() {
            return Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned());
        }
        let said = said(&output);
        Err(format!(
            "git {} failed{}",
            args.first().copied().unwrap_or_default(),
            if said.is_empty() {
                String::new()
            } else {
                format!(": {said}")
            }
        ))
    }
}

/// The end of what a command wrote, which is where its error usually is.
pub fn tail(text: &str) -> String {
    let text = text.trim();
    let count = text.chars().count();
    if count <= TAIL_CHARS {
        return text.to_owned();
    }
    let kept: String = text.chars().skip(count - TAIL_CHARS).collect();
    format!("…{kept}")
}

/// The end of what a command said on both its outputs.
pub fn said(output: &Output) -> String {
    let mut text = String::from_utf8_lossy(&output.stderr).into_owned();
    let out = String::from_utf8_lossy(&output.stdout);
    if !out.trim().is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&out);
    }
    tail(&text)
}

/// The top folder of the main checkout of the repository `path` is in, whether `path` is in
/// that checkout or in one of its linked worktrees. Read from git's own files, so no git runs.
pub fn main_checkout(path: &std::path::Path) -> Option<std::path::PathBuf> {
    let root = sikemux_process::files::repo_root(path)?;
    let dot_git = root.join(".git");
    if dot_git.is_dir() {
        return Some(root.to_path_buf());
    }
    let link = std::fs::read_to_string(&dot_git).ok()?;
    let gitdir = root.join(link.strip_prefix("gitdir:")?.trim());
    let common = std::fs::read_to_string(gitdir.join("commondir")).ok()?;
    let common = std::fs::canonicalize(gitdir.join(common.trim())).ok()?;
    if common.file_name()? != ".git" {
        return None;
    }
    common.parent().map(std::path::Path::to_path_buf)
}

/// Where `repo`'s remote named origin is, when it has one.
pub fn origin(repo: &std::path::Path) -> Option<String> {
    Git::quick(repo)
        .trimmed(&["remote", "get-url", "origin"])
        .ok()
        .filter(|url| !url.is_empty())
}

fn run_git(repo: &str, args: &[&str]) -> Result<(bool, String, String), String> {
    let out = Git::at(repo).output(args)?;
    Ok((
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    ))
}

fn git_ok(repo: &str, args: &[&str]) -> Result<String, String> {
    let (ok, so, se) = run_git(repo, args)?;
    if ok {
        Ok(so)
    } else {
        Err(if se.trim().is_empty() { so } else { se })
    }
}

fn git_has_head(repo: &str) -> bool {
    git_ok(repo, &["rev-parse", "--verify", "HEAD"]).is_ok()
}

fn current_branch_name(repo: &str) -> Result<String, String> {
    let branch = git_ok(repo, &["branch", "--show-current"])?
        .trim()
        .to_string();
    if branch.is_empty() {
        Err("Cannot operate on a detached HEAD — checkout a branch first.".into())
    } else {
        Ok(branch)
    }
}

fn remote_names(repo: &str) -> Result<Vec<String>, String> {
    Ok(git_ok(repo, &["remote"])?
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

fn default_remote(repo: &str) -> Result<String, String> {
    let remotes = remote_names(repo)?;
    if remotes.iter().any(|r| r == "origin") {
        return Ok("origin".into());
    }
    if remotes.len() == 1 {
        return Ok(remotes[0].clone());
    }
    if remotes.is_empty() {
        Err("No git remotes configured — add a remote before publishing this branch.".into())
    } else {
        Err(format!(
            "No remote named origin. Pick/set an upstream from the remotes panel. Available remotes: {}",
            remotes.join(", ")
        ))
    }
}

fn has_upstream(repo: &str) -> bool {
    git_ok(
        repo,
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
    )
    .map(|s| !s.trim().is_empty())
    .unwrap_or(false)
}

fn path_in_index(repo: &str, path: &str) -> bool {
    run_git(repo, &["ls-files", "--error-unmatch", "--", path])
        .map(|(ok, so, _)| ok && !so.trim().is_empty())
        .unwrap_or(false)
}

fn path_in_head(repo: &str, path: &str) -> bool {
    run_git(repo, &["ls-tree", "-r", "--name-only", "HEAD", "--", path])
        .map(|(ok, so, _)| ok && so.lines().any(|line| line == path))
        .unwrap_or(false)
}

/// Which of these paths, inside the repository at `repo`, git ignores, by git's own rules read in
/// this process rather than by asking git, so a folder of thousands lists at once. A folder's path
/// may end in `/`, which saves looking at the disk. A file git tracks is not ignored, as git says.
/// A folder that is not in a repository ignores nothing.
pub fn ignored_paths(repo: &str, paths: &[String]) -> std::collections::HashSet<String> {
    let Ok(repository) = Repository::open(repo) else {
        return Default::default();
    };
    let root = std::path::Path::new(repo);
    let mut index = None;
    paths
        .iter()
        .filter(|path| {
            let Ok(relative) = std::path::Path::new(path.as_str()).strip_prefix(root) else {
                return false;
            };
            let folder = path.ends_with('/');
            let asked = if folder {
                format!("{}/", relative.to_string_lossy())
            } else {
                relative.to_string_lossy().into_owned()
            };
            if !repository.is_path_ignored(&asked).unwrap_or(false) {
                return false;
            }
            if folder {
                return true;
            }
            let index = index.get_or_insert_with(|| repository.index().ok());
            index
                .as_ref()
                .is_none_or(|index| index.get_path(relative, 0).is_none())
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::branches::{git_branch_delete, git_checkout, git_merge, git_reset};
    use super::remote::git_delete_remote_branch;
    use super::revisions::git_show;
    use super::*;
    use std::{fs, path::Path};
    use tempfile::tempdir;

    #[test]
    fn ignored_paths_names_what_gitignore_covers_and_nothing_outside_a_repo() {
        let dir = tempdir().unwrap();
        let repo = dir.path();
        git(repo, &["init", "-q"]);
        fs::write(repo.join(".gitignore"), "*.log\nnode_modules/\n").unwrap();
        fs::create_dir_all(repo.join("node_modules/pkg")).unwrap();
        fs::write(repo.join("debug.log"), "").unwrap();
        fs::write(repo.join("debug.txt"), "").unwrap();
        fs::write(repo.join("node_modules/pkg/index.js"), "").unwrap();
        let path = |name: &str| repo.join(name).to_string_lossy().into_owned();
        let listed = [
            "debug.log",
            "debug.txt",
            "node_modules",
            "node_modules/pkg/index.js",
        ]
        .map(path);
        let ignored = ignored_paths(&repo_arg(repo), &listed);
        assert!(ignored.contains(&path("debug.log")));
        assert!(ignored.contains(&path("node_modules")));
        assert!(ignored.contains(&path("node_modules/pkg/index.js")));
        assert!(!ignored.contains(&path("debug.txt")));

        let outside = tempdir().unwrap();
        let loose = outside.path().join("a.txt").to_string_lossy().into_owned();
        assert!(ignored_paths(&repo_arg(outside.path()), &[loose]).is_empty());
    }

    pub(super) fn repo_arg(repo: &Path) -> String {
        repo.to_string_lossy().into_owned()
    }

    /// Test repositories commit without signing, whatever the person's own Git config asks for.
    const UNSIGNED: [&str; 4] = ["-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"];

    pub(super) fn git(repo: &Path, args: &[&str]) -> String {
        let out = sikemux_process::user_environment::command("git")
            .args(UNSIGNED)
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "git {:?}\nstdout:\n{}\nstderr:\n{}",
            args,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    pub(super) fn git_at(repo: &Path, stamp: &str, args: &[&str]) -> String {
        let out = sikemux_process::user_environment::command("git")
            .args(UNSIGNED)
            .arg("-C")
            .arg(repo)
            .args(args)
            .env("GIT_AUTHOR_DATE", stamp)
            .env("GIT_COMMITTER_DATE", stamp)
            .output()
            .expect("run git");
        assert!(out.status.success(), "git {args:?}");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    pub(super) fn init_repo() -> tempfile::TempDir {
        let td = tempdir().expect("tempdir");
        git(td.path(), &["init"]);
        git(td.path(), &["config", "user.email", "sikemux@example.test"]);
        git(td.path(), &["config", "user.name", "sikemux"]);
        git(td.path(), &["config", "core.autocrlf", "false"]);
        td
    }

    #[cfg(unix)]
    fn run(command: &mut Command, input: Option<&[u8]>) -> Result<Output, String> {
        let wait = Duration::from_secs(5);
        sikemux_process::run(command, input, wait, MAX_COMMAND_OUTPUT_BYTES, None)
            .map_err(|error| error.to_string())
    }

    #[test]
    fn subprocess_drains_output_before_child_reads_stdin() {
        let input = vec![b'i'; 2 * 1024 * 1024];
        let mut command = sikemux_process::user_environment::command("sh");
        command.args([
            "-c",
            "dd if=/dev/zero bs=1048576 count=2 2>/dev/null; cat >/dev/null",
        ]);
        let out = run(&mut command, Some(&input))
            .expect("concurrent stdin/stdout handling must not deadlock");
        assert!(out.status.success());
        assert_eq!(out.stdout.len(), 2 * 1024 * 1024);
    }

    #[cfg(unix)]
    #[test]
    fn subprocess_output_is_bounded() {
        let mut command = sikemux_process::user_environment::command("sh");
        command.args(["-c", "dd if=/dev/zero bs=1048576 count=33 2>/dev/null"]);
        let error = run(&mut command, None).expect_err("oversized output must be rejected");
        assert!(error.contains("output exceeds"), "{error}");
    }

    pub(super) fn commit_base(repo: &Path) {
        fs::write(repo.join("f.txt"), "base\n").expect("write base");
        git(repo, &["add", "f.txt"]);
        git(repo, &["commit", "-m", "base"]);
    }

    #[tokio::test]
    async fn refs_that_look_like_options_are_only_ever_refs() {
        let td = init_repo();
        commit_base(td.path());
        let repo = repo_arg(td.path());
        let main = git(td.path(), &["branch", "--show-current"])
            .trim()
            .to_string();
        git(td.path(), &["update-ref", "refs/heads/-q", "HEAD"]);

        git_checkout(repo.clone(), "-q".into())
            .await
            .expect("checkout");
        assert_eq!(git(td.path(), &["branch", "--show-current"]).trim(), "-q");
        git_checkout(repo.clone(), main)
            .await
            .expect("checkout back");
        git_merge(repo.clone(), "-q".into()).await.expect("merge");
        git_branch_delete(repo.clone(), "-q".into(), true)
            .await
            .expect("delete");
        assert!(Repository::open(td.path())
            .expect("open")
            .find_reference("refs/heads/-q")
            .is_err());

        let written = td.path().join("written");
        let flag = format!("--output={}", written.display());
        assert!(git_show(repo.clone(), flag.clone()).await.is_err());
        assert!(git_reset(repo.clone(), flag.clone(), "soft".into())
            .await
            .is_err());
        assert!(!written.exists());

        let remote = tempdir().expect("remote");
        git(remote.path(), &["init", "--bare"]);
        git(
            td.path(),
            &["remote", "add", "origin", &repo_arg(remote.path())],
        );
        git(td.path(), &["push", "origin", "HEAD:refs/heads/-q"]);
        git_delete_remote_branch(repo.clone(), "origin".into(), "-q".into())
            .await
            .expect("delete remote branch");
        assert!(Repository::open(remote.path())
            .expect("open remote")
            .find_reference("refs/heads/-q")
            .is_err());
    }
}
