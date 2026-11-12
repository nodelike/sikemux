//! Throwaway repositories for tests, here and in the crates built on this one.

use std::fs;
use std::path::Path;

pub fn repo_arg(repo: &Path) -> String {
    repo.to_string_lossy().into_owned()
}

/// Test repositories commit without signing, whatever the person's own Git config asks for.
const UNSIGNED: [&str; 4] = ["-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"];

pub fn git(repo: &Path, args: &[&str]) -> String {
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

pub fn git_at(repo: &Path, stamp: &str, args: &[&str]) -> String {
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

pub fn init_repo() -> tempfile::TempDir {
    let td = tempfile::tempdir().expect("tempdir");
    git(td.path(), &["init"]);
    git(td.path(), &["config", "user.email", "sikemux@example.test"]);
    git(td.path(), &["config", "user.name", "sikemux"]);
    git(td.path(), &["config", "core.autocrlf", "false"]);
    td
}

pub fn commit_base(repo: &Path) {
    fs::write(repo.join("f.txt"), "base\n").expect("write base");
    git(repo, &["add", "f.txt"]);
    git(repo, &["commit", "-m", "base"]);
}
