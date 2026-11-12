//! What a piece of work is, as a gate's yes covers it: the path and content of everything it
//! changes against the branch it lands on, committed or not, untracked files included. A
//! rebase that changes nothing of it keeps it. Work that changes nothing against that branch,
//! as a clean checkout of it, is the tree it holds, so each commit there is new work.

use std::path::Path;

use sha2::{Digest, Sha256};

use crate::git::Git;

/// More untracked content than this is not read, and the work then has no fingerprint.
const MAX_UNTRACKED_BYTES: u64 = 64 * 1024 * 1024;

/// A fingerprint of the work checked out at `cwd`, or none when git can't say what it is: a
/// folder that isn't a repository, a remote whose default branch isn't known, any git
/// failure, or too much untracked content to read.
pub(crate) fn of(cwd: &Path) -> Option<String> {
    let git = || Git::quick(cwd);
    let tree = git().trimmed(&["rev-parse", "HEAD^{tree}"]).ok()?;
    let landing = ["origin/HEAD", "@{upstream}", "origin/main", "origin/master"]
        .into_iter()
        .find_map(|branch| git().trimmed(&["merge-base", "HEAD", branch]).ok());
    let mut digest = Sha256::new();
    let changed = match landing {
        Some(base) => git()
            .trimmed(&[
                "diff",
                "--binary",
                "--full-index",
                "--no-color",
                "--no-ext-diff",
                "--no-renames",
                &base,
            ])
            .ok()?,
        None => {
            let tree = git().trimmed(&["ls-files", "--stage", "--", ":/"]).ok()?;
            digest.update(tree.as_bytes());
            git()
                .trimmed(&[
                    "diff",
                    "--binary",
                    "--full-index",
                    "--no-color",
                    "--no-ext-diff",
                    "HEAD",
                ])
                .ok()?
        }
    };
    for line in changed.lines() {
        if line.starts_with("index ") {
            let after = line.split_whitespace().nth(1)?;
            digest.update(after.split("..").nth(1)?.as_bytes());
        } else if !line.starts_with("@@") {
            digest.update(line.as_bytes());
        }
        digest.update(b"\n");
    }
    let untracked = git()
        .trimmed(&["ls-files", "--others", "--exclude-standard", "--", ":/"])
        .ok()?;
    let mut read = 0;
    for path in untracked.lines().filter(|path| !path.is_empty()) {
        let file = cwd.join(path);
        let size = std::fs::metadata(&file).ok()?.len();
        read += size;
        if read > MAX_UNTRACKED_BYTES {
            return None;
        }
        digest.update(path.as_bytes());
        digest.update(b"\0");
        digest.update(std::fs::read(&file).ok()?);
        digest.update(b"\0");
    }
    if changed.trim().is_empty() {
        digest.update(b"tree ");
        digest.update(tree.as_bytes());
    }
    Some(hex::encode(digest.finalize()).get(..20)?.to_owned())
}

#[cfg(test)]
mod tests {
    use super::super::push::tests::git;
    use super::*;

    /// A clone on branch `work` with one committed change to `app.txt` against main.
    fn checkout() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let origin = dir.path().join("origin.git");
        let work = dir.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        git(
            dir.path(),
            &["init", "--bare", "-b", "main", origin.to_str().unwrap()],
        );
        git(&work, &["init", "-b", "main"]);
        std::fs::write(work.join("app.txt"), "one\ntwo\nthree\n").unwrap();
        std::fs::write(work.join("other.txt"), "a\n").unwrap();
        git(&work, &["add", "."]);
        git(&work, &["commit", "-m", "first"]);
        git(
            &work,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        git(&work, &["push", "-u", "origin", "main"]);
        git(&work, &["remote", "set-head", "origin", "main"]);
        git(&work, &["checkout", "-b", "work"]);
        std::fs::write(work.join("app.txt"), "one\ntwo\nthree\nfour\n").unwrap();
        git(&work, &["commit", "-am", "four"]);
        dir
    }

    #[test]
    fn a_rebase_that_changes_nothing_of_the_work_keeps_it() {
        let dir = checkout();
        let work = dir.path().join("work");
        let before = of(&work).unwrap();
        git(&work, &["checkout", "main"]);
        std::fs::write(work.join("other.txt"), "b\n").unwrap();
        git(&work, &["commit", "-am", "elsewhere"]);
        git(&work, &["push", "origin", "main"]);
        git(&work, &["fetch", "origin"]);
        git(&work, &["checkout", "work"]);
        git(&work, &["rebase", "origin/main"]);
        assert_eq!(of(&work).unwrap(), before);
    }

    /// The first fingerprint read untracked files by name and dropped binary and moved lines.
    #[test]
    fn every_change_to_the_work_changes_it() {
        let dir = checkout();
        let work = dir.path().join("work");
        let mut seen = vec![of(&work).unwrap()];
        let mut changed = |change: &dyn Fn()| {
            change();
            let now = of(&work).unwrap();
            assert!(!seen.contains(&now));
            seen.push(now);
        };
        changed(&|| std::fs::write(work.join("new.txt"), "x").unwrap());
        changed(&|| std::fs::write(work.join("new.txt"), "y").unwrap());
        changed(&|| std::fs::write(work.join("app.txt"), "one\ntwo\nthree\nfour\nfive\n").unwrap());
        changed(&|| std::fs::write(work.join("app.txt"), "four\none\ntwo\nthree\nfive\n").unwrap());
        changed(&|| std::fs::write(work.join("blob.bin"), [0u8, 1, 2, 255]).unwrap());
        changed(&|| {
            git(&work, &["add", "blob.bin"]);
            git(&work, &["commit", "-qm", "blob"]);
            std::fs::write(work.join("blob.bin"), [0u8, 1, 2, 254]).unwrap();
        });
    }

    /// A repository whose remote never said its default branch had no fingerprint, so each
    /// retry of a failed deploy there raised a new call.
    #[test]
    fn without_a_repository_there_is_no_fingerprint_and_without_a_default_branch_there_is() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(of(dir.path()), None);
        let dir = checkout();
        let work = dir.path().join("work");
        git(&work, &["remote", "set-head", "origin", "--delete"]);
        let first = of(&work).unwrap();
        assert_eq!(of(&work).unwrap(), first);
        let alone = tempfile::tempdir().unwrap();
        git(alone.path(), &["init", "-b", "main"]);
        std::fs::write(alone.path().join("a.txt"), "a").unwrap();
        git(alone.path(), &["add", "."]);
        git(alone.path(), &["commit", "-m", "a"]);
        let before = of(alone.path()).unwrap();
        std::fs::write(alone.path().join("a.txt"), "b").unwrap();
        assert_ne!(of(alone.path()).unwrap(), before);
    }

    /// Untracked files were read only under the command's folder.
    #[test]
    fn a_new_file_anywhere_in_the_repository_changes_the_work() {
        let dir = checkout();
        let work = dir.path().join("work");
        std::fs::create_dir_all(work.join("apps/web")).unwrap();
        std::fs::create_dir_all(work.join("db/migrations")).unwrap();
        let inside = work.join("apps/web");
        let before = of(&inside).unwrap();
        std::fs::write(work.join("db/migrations/005.sql"), "drop table users;").unwrap();
        assert_ne!(of(&inside).unwrap(), before);
    }
}
