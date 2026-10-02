pub mod ai;
mod ai_provider;
pub mod blame;
pub mod branches;
pub mod changes;
pub mod commit;
pub mod log;
pub mod remote;
pub mod revisions;
pub mod stash;
pub mod status;
pub mod worktree;

use std::io::{self, Read};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use git2::Repository;
use tauri::async_runtime::spawn_blocking;

/// Run a synchronous closure off the Tauri worker pool. Every `pub fn`
/// command in this module used to block the worker thread while libgit2
/// walked the repo; with many projects open + an fs-watch storm, that
/// pool gets saturated and unrelated IPC (PTY input, etc.) stalls.
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

fn open_repo(path: &str) -> Result<Repository, String> {
    Repository::discover(path).map_err(|e| e.message().to_string())
}

const GIT_COMMAND_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_COMMAND_OUTPUT_BYTES: usize = 32 * 1024 * 1024;

fn kill_and_reap_process(child: &mut std::process::Child) {
    #[cfg(unix)]
    // SAFETY: kill only takes integers and touches no memory. A spawned child's id is
    // never zero, so this never becomes kill(0), which would signal our own process group.
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

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

/// Capture both pipes while enforcing a deadline and an output cap. Stdin and
/// both output pipes are handled concurrently so no pipe-ordering deadlock is
/// possible. Every timeout/error path kills the process group.
fn run_command_with_timeout(
    command: &mut Command,
    input: Option<&[u8]>,
    timeout: Duration,
) -> Result<Output, String> {
    crate::bounded_process::run(command, input, timeout, MAX_COMMAND_OUTPUT_BYTES, None)
        .map_err(|error| error.to_string())
}

/// Every git process the app starts goes through here. A repo's own config can
/// name an fsmonitor program, which git would otherwise run on any status read.
pub(crate) fn git_command(repo: &str) -> Command {
    let mut command = sikemux_process::user_environment::command("git");
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(["-c", "core.fsmonitor=false", "-C"])
        .arg(repo);
    command
}

fn run_git(repo: &str, args: &[&str]) -> Result<(bool, String, String), String> {
    let mut command = git_command(repo);
    command.args(args);
    let out = run_command_with_timeout(&mut command, None, GIT_COMMAND_TIMEOUT)?;
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

#[cfg(test)]
mod tests {
    use super::branches::{git_branch_delete, git_checkout, git_merge, git_reset};
    use super::remote::git_delete_remote_branch;
    use super::revisions::git_show;
    use super::*;
    use std::{fs, path::Path};
    use tempfile::tempdir;

    pub(super) fn repo_arg(repo: &Path) -> String {
        repo.to_string_lossy().into_owned()
    }

    pub(super) fn git(repo: &Path, args: &[&str]) -> String {
        let out = sikemux_process::user_environment::command("git")
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
    #[test]
    fn subprocess_drains_output_before_child_reads_stdin() {
        let input = vec![b'i'; 2 * 1024 * 1024];
        let mut command = sikemux_process::user_environment::command("sh");
        command.args([
            "-c",
            "dd if=/dev/zero bs=1048576 count=2 2>/dev/null; cat >/dev/null",
        ]);
        let out = run_command_with_timeout(&mut command, Some(&input), Duration::from_secs(5))
            .expect("concurrent stdin/stdout handling must not deadlock");
        assert!(out.status.success());
        assert_eq!(out.stdout.len(), 2 * 1024 * 1024);
    }

    #[cfg(unix)]
    #[test]
    fn subprocess_output_is_bounded() {
        let mut command = sikemux_process::user_environment::command("sh");
        command.args(["-c", "dd if=/dev/zero bs=1048576 count=33 2>/dev/null"]);
        let error = run_command_with_timeout(&mut command, None, Duration::from_secs(5))
            .expect_err("oversized output must be rejected");
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
