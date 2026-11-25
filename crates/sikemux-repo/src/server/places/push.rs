//! Where a `git push` lands, read as git resolves it: its remote, its refspecs or what
//! `push.default` and `@{push}` make of none, and whether each source and destination is a
//! branch or a tag. Whatever this can't prove stays off the project's own branch, a release
//! branch and every tag counts as reaching them.

use std::path::{Path, PathBuf};

use super::place::says_production;
use crate::git::Git;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reach {
    /// Only branches other than the project's own, a release branch or one named production.
    Branches,
    /// The project's branch, or what can't be proved to stay off it.
    Project,
    /// Tags, which can start a release, by name, and whatever else the push reaches, the
    /// project's branch included. No names when which tags can't be read.
    Tags(Vec<String>),
    /// A remote that isn't the project's own code host, such as Heroku or Dokku: a deploy to
    /// the app the remote names.
    Deploy { tool: String, app: String },
}

/// Code hosts a remote of the project's own lives on.
const CODE_HOSTS: [&str; 6] = [
    "github.com",
    "gitlab.com",
    "bitbucket.org",
    "codeberg.org",
    "gitea.com",
    "git.sr.ht",
];
/// Long options of `git push` that change nothing of where it goes, spelled out in full: git
/// takes any unique prefix of an option, so anything else can't be read.
const SWITCHES: [&str; 21] = [
    "--force",
    "--force-if-includes",
    "--no-force-if-includes",
    "--set-upstream",
    "--no-verify",
    "--verify",
    "--dry-run",
    "--porcelain",
    "--progress",
    "--no-progress",
    "--quiet",
    "--verbose",
    "--atomic",
    "--no-atomic",
    "--thin",
    "--no-thin",
    "--ipv4",
    "--ipv6",
    "--no-signed",
    "--no-recurse-submodules",
    "--no-follow-tags",
];
/// Long options of `git push` that take a value after `=` and change nothing of where it goes.
const VALUED: [&str; 3] = ["--push-option", "--signed", "--recurse-submodules"];

/// Branch names that are a project's own or a release's whatever the remote's default is.
const KEPT_BRANCHES: [&str; 9] = [
    "main",
    "master",
    "develop",
    "development",
    "trunk",
    "stable",
    "gh-pages",
    "production",
    "live",
];

/// The folder git runs the command `what` in when started from `cwd`, as its `-C` options
/// move it.
pub fn run_in(what: &str, cwd: &Path) -> PathBuf {
    let words: Vec<&str> = what.split_whitespace().collect();
    let mut dir = cwd.to_path_buf();
    if !words.first().is_some_and(|program| is_git(program)) {
        return dir;
    }
    let mut rest = words.get(1..).unwrap_or_default();
    loop {
        match rest {
            ["-C", moved, after @ ..] => {
                dir = dir.join(moved);
                rest = after;
            }
            ["-c", _, after @ ..] => rest = after,
            _ => return dir,
        }
    }
}

/// The commits the tags `names` name in the checkout at `cwd`, so that moving a tag onto other
/// work makes its push new work. None when any of them can't be read.
pub fn tag_commits(names: &[String], cwd: &Path) -> Option<String> {
    let commits: Option<Vec<String>> = (names.iter())
        .map(|name| {
            Git::quick(cwd)
                .trimmed(&[
                    "rev-parse",
                    "--verify",
                    &format!("refs/tags/{name}^{{commit}}"),
                ])
                .ok()
        })
        .collect();
    commits
        .filter(|commits| !commits.is_empty())
        .map(|commits| commits.join(" "))
}

fn is_git(program: &str) -> bool {
    program.rsplit('/').next() == Some("git")
}

/// Where the push `what`, run from `cwd`, lands, judged ref by ref: any tag it carries is
/// judged as a tag pushed alone would be.
pub fn reach(what: &str, cwd: &Path) -> Reach {
    let unknown = || Reach::Tags(Vec::new());
    let words: Vec<&str> = what.split_whitespace().collect();
    let Some((_, rest)) = words.split_first().filter(|(program, _)| is_git(program)) else {
        return unknown();
    };
    let mut rest = rest;
    while let ["-C", _, after @ ..] = rest {
        rest = after;
    }
    let cwd = &run_in(what, cwd);
    let Some(("push", args)) = rest.split_first().map(|(push, args)| (*push, args)) else {
        return unknown();
    };
    let git = || Git::quick(cwd);
    let mut positional: Vec<&str> = Vec::new();
    let mut tags = false;
    let mut deleting = false;
    let mut at = 0;
    while let Some(arg) = args.get(at) {
        at += 1;
        match *arg {
            "-o" | "--push-option" | "--receive-pack" | "--exec" => at += 1,
            "--tags" | "--follow-tags" => tags = true,
            "--delete" => deleting = true,
            arg if arg.starts_with("--recurse-submodules=") => {
                if !arg.ends_with("=no") && !arg.ends_with("=check") {
                    return unknown();
                }
            }
            arg if arg.starts_with("--") => {
                let name = arg.split_once('=').map_or(arg, |(name, _)| name);
                let valued = arg.contains('=');
                let known = SWITCHES.contains(&name)
                    || (valued && VALUED.contains(&name))
                    || name == "--force-with-lease";
                if !known {
                    return unknown();
                }
            }
            arg if arg.starts_with('-') => {
                let letters = arg.get(1..).unwrap_or_default();
                if letters.starts_with('o') {
                    continue;
                }
                if !letters.chars().all(|letter| "funqv46d".contains(letter)) {
                    return unknown();
                }
                deleting |= letters.contains('d');
            }
            arg => positional.push(arg),
        }
    }
    let (remote, refspecs) = match positional.split_first() {
        Some((remote, refspecs)) => ((*remote).to_owned(), refspecs.to_vec()),
        None => match push_remote(cwd) {
            Some(remote) => (remote, Vec::new()),
            None => return unknown(),
        },
    };
    if let Some(deploy) = deploy_remote(&remote, cwd) {
        return deploy;
    }
    let mirror = git().trimmed(&["config", "--bool", &format!("remote.{remote}.mirror")]);
    if mirror.is_ok_and(|said| said == "true") {
        return unknown();
    }
    let reaches = match refspecs.is_empty() {
        true => {
            let pushes = git().trimmed(&["config", "--get-all", &format!("remote.{remote}.push")]);
            let default = git()
                .trimmed(&["config", "push.default"])
                .unwrap_or_default();
            if pushes.is_ok_and(|pushes| !pushes.is_empty()) {
                return unknown();
            }
            if default == "matching" {
                vec![Reach::Project]
            } else if default == "nothing" {
                vec![Reach::Branches]
            } else {
                match pushed_to(cwd) {
                    Some(branch) => vec![branch_reach(&branch, &remote, cwd)],
                    None => return unknown(),
                }
            }
        }
        false => (refspecs.iter())
            .map(|refspec| match deleting {
                true => refspec_reach(&format!(":{refspec}"), &remote, cwd),
                false => refspec_reach(refspec, &remote, cwd),
            })
            .collect(),
    };
    let follows = git()
        .trimmed(&["config", "--bool", "push.followTags"])
        .is_ok_and(|said| said == "true");
    let mut carries_tags = tags || follows;
    let mut unnamed = tags || follows;
    let mut names = Vec::new();
    for reached in &reaches {
        if let Reach::Tags(given) = reached {
            carries_tags = true;
            unnamed |= given.is_empty();
            names.extend(given.iter().cloned());
        }
    }
    if carries_tags {
        return Reach::Tags(if unnamed { Vec::new() } else { names });
    }
    match reaches.contains(&Reach::Project) {
        true => Reach::Project,
        false => Reach::Branches,
    }
}

/// The pull request a `gh pr merge` merges, as its words name it.
pub struct Merging {
    /// Its number, address or branch; none for the current branch's.
    pull: Option<String>,
    /// The `owner/name` of its repository, when the words name one.
    pub repo: Option<String>,
    /// Whether every word was understood, so that `pull` is the one merged.
    read: bool,
}

pub fn merging(what: &str) -> Merging {
    let mut found = Merging {
        pull: None,
        repo: None,
        read: false,
    };
    let words: Vec<&str> = what.split_whitespace().collect();
    let args = match words.as_slice() {
        [gh, "pr", "merge", args @ ..] if gh.rsplit('/').next() == Some("gh") => args,
        _ => return found,
    };
    found.read = true;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match *arg {
            "-R" | "--repo" => found.repo = args.next().map(|repo| (*repo).to_owned()),
            "--match-head-commit" => _ = args.next(),
            "-m" | "--merge" | "-s" | "--squash" | "-r" | "--rebase" | "-d" | "--delete-branch"
            | "--auto" | "--admin" => {}
            arg if arg.starts_with("--match-head-commit=") => {}
            arg if arg.starts_with("--repo=") => {
                found.repo = arg.split_once('=').map(|(_, repo)| repo.to_owned());
            }
            arg if arg.starts_with('-') || found.pull.is_some() => found.read = false,
            arg => found.pull = Some(arg.to_owned()),
        }
    }
    let address = found.pull.as_deref().filter(|pull| pull.contains("://"));
    if let Some((repo, _)) = address.and_then(|address| address.split_once("/pull/")) {
        found.repo = Some(repo.to_owned());
    }
    found
}

impl Merging {
    /// Whether the pull request is of the repository checked out at `root`.
    pub fn is_into(&self, root: &Path) -> bool {
        let Some(repo) = &self.repo else {
            return true;
        };
        let mut named = repo.trim_end_matches(".git").rsplit('/');
        let (Some(name), Some(owner)) = (named.next(), named.next()) else {
            return true;
        };
        let remotes = Git::quick(root)
            .trimmed(&["remote", "-v"])
            .unwrap_or_default();
        (remotes.lines())
            .filter_map(|line| line.split_whitespace().nth(1))
            .filter_map(crate::server::credential::owner_and_name)
            .any(|(theirs, its)| {
                theirs.eq_ignore_ascii_case(owner) && its.eq_ignore_ascii_case(name)
            })
    }

    /// Where a merge run from `cwd` lands in the repository checked out at `root`, and the
    /// commit it merges. GitHub is asked for the branch it goes into.
    pub fn reach(&self, root: &Path, cwd: &Path) -> (Reach, Option<String>) {
        if !self.read {
            return (Reach::Project, None);
        }
        let mut view = sikemux_process::user_environment::command("gh");
        view.current_dir(cwd)
            .env("GH_PROMPT_DISABLED", "1")
            .args(["pr", "view"])
            .args(&self.pull)
            .args(["--json", "baseRefName,headRefOid"]);
        if let Some(repo) = &self.repo {
            view.args(["--repo", repo]);
        }
        let found = (view.output().ok())
            .filter(|output| output.status.success())
            .and_then(|output| serde_json::from_slice::<serde_json::Value>(&output.stdout).ok());
        let field = |name: &str| found.as_ref()?.get(name)?.as_str().map(str::to_owned);
        let reach =
            field("baseRefName").map_or(Reach::Project, |base| branch_reach(&base, "origin", root));
        (reach, field("headRefOid"))
    }
}

/// The remote a push with none named goes to, from `@{push}`.
fn push_remote(cwd: &Path) -> Option<String> {
    let full = Git::quick(cwd)
        .trimmed(&["rev-parse", "--symbolic-full-name", "@{push}"])
        .ok()?;
    let rest = full.strip_prefix("refs/remotes/")?;
    rest.split_once('/').map(|(remote, _)| remote.to_owned())
}

/// The branch a push with no refspec updates, from `@{push}`.
fn pushed_to(cwd: &Path) -> Option<String> {
    let full = Git::quick(cwd)
        .trimmed(&["rev-parse", "--symbolic-full-name", "@{push}"])
        .ok()?;
    let rest = full.strip_prefix("refs/remotes/")?;
    rest.split_once('/').map(|(_, branch)| branch.to_owned())
}

/// The deploy a push to `remote` is, when the remote isn't on the host the project's own
/// code lives on.
fn deploy_remote(remote: &str, cwd: &Path) -> Option<Reach> {
    let git = || Git::quick(cwd);
    let url = match remote.contains("://") || remote.contains('@') {
        true => remote.to_owned(),
        false => git()
            .trimmed(&["remote", "get-url", "--push", remote])
            .ok()?,
    };
    let host = host_of(&url)?;
    if let Some(app) = url.split_once("git.heroku.com/").map(|(_, app)| app) {
        let app = app.trim_end_matches('/').trim_end_matches(".git");
        return Some(Reach::Deploy {
            tool: "Heroku".to_owned(),
            app: app.to_owned(),
        });
    }
    let own = (git()
        .trimmed(&["remote", "get-url", "--push", "origin"])
        .ok())
    .and_then(|origin| host_of(&origin));
    if CODE_HOSTS.contains(&host.as_str()) || own.as_deref() == Some(host.as_str()) {
        return None;
    }
    let app = url
        .rsplit([':', '/'])
        .find(|part| !part.is_empty())
        .unwrap_or(remote)
        .trim_end_matches(".git");
    let tool = match url.starts_with("dokku@") {
        true => "Dokku".to_owned(),
        false => format!("the {remote} remote"),
    };
    Some(Reach::Deploy {
        tool,
        app: app.to_owned(),
    })
}

fn host_of(url: &str) -> Option<String> {
    if let Ok(parsed) = url::Url::parse(url) {
        if let Some(host) = parsed.host_str() {
            return Some(host.to_owned());
        }
    }
    let (before, _) = url.split_once(':')?;
    let host = before.rsplit('@').next()?;
    (!host.is_empty() && !host.contains('/')).then(|| host.to_owned())
}

fn refspec_reach(refspec: &str, remote: &str, cwd: &Path) -> Reach {
    let refspec = refspec.trim_start_matches('+');
    if refspec.contains('*') {
        return Reach::Tags(Vec::new());
    }
    let (source, destination) = match refspec.split_once(':') {
        Some((source, destination)) => (source, Some(destination)),
        None => (refspec, None),
    };
    let git = || Git::quick(cwd);
    let is_ref = |full: &str| {
        git()
            .trimmed(&["show-ref", "--verify", "--quiet", full])
            .is_ok()
    };
    if source.is_empty() {
        return deleted_reach(destination.unwrap_or_default(), remote, cwd, is_ref);
    }
    let source = match source {
        "HEAD" | "@" => match git().trimmed(&["symbolic-ref", "--short", "HEAD"]) {
            Ok(branch) => branch,
            Err(_) if destination.is_some() => source.to_owned(),
            Err(_) => return Reach::Project,
        },
        source => source.to_owned(),
    };
    let resolved = git()
        .trimmed(&["rev-parse", "--symbolic-full-name", &source])
        .unwrap_or_default();
    if outside_branches_and_tags(destination.unwrap_or(&source)) {
        return Reach::Branches;
    }
    let tag_source = source.starts_with("refs/tags/")
        || resolved.starts_with("refs/tags/")
        || (!source.starts_with("refs/")
            && is_ref(&format!("refs/tags/{source}"))
            && !is_ref(&format!("refs/heads/{source}")));
    let short = |name: &str| {
        let name = name.strip_prefix("refs/tags/").unwrap_or(name);
        name.strip_prefix("tags/").unwrap_or(name).to_owned()
    };
    if tag_source {
        let name = destination.map_or_else(|| short(&resolved_or(&resolved, &source)), short);
        return Reach::Tags(vec![name]);
    }
    let destination = destination.unwrap_or(&source);
    if destination.is_empty() {
        return Reach::Project;
    }
    if destination.starts_with("refs/tags/") || destination.starts_with("tags/") {
        return Reach::Tags(vec![short(destination)]);
    }
    let branch = (destination.strip_prefix("refs/heads/"))
        .or_else(|| destination.strip_prefix("heads/"))
        .unwrap_or(destination);
    if branch.starts_with("refs/") {
        return Reach::Project;
    }
    if is_ref(&format!("refs/tags/{branch}")) {
        return Reach::Tags(vec![branch.to_owned()]);
    }
    branch_reach(branch, remote, cwd)
}

/// Whether the full name of a ref puts it in a folder of its own, such as `refs/notes/x`, where
/// no release or branch lives. A push to `refs/for/` is Gerrit's way to a branch.
fn outside_branches_and_tags(name: &str) -> bool {
    (name.strip_prefix("refs/"))
        .and_then(|rest| rest.split_once('/'))
        .is_some_and(|(folder, _)| {
            !["", "heads", "tags", "for"]
                .iter()
                .any(|kept| folder.eq_ignore_ascii_case(kept))
        })
}

/// Where deleting `destination` on `remote` reaches: a branch or a tag as its full name or
/// what the checkout last saw of the remote says, else whatever it could be.
fn deleted_reach(
    destination: &str,
    remote: &str,
    cwd: &Path,
    is_ref: impl Fn(&str) -> bool,
) -> Reach {
    if let Some(tag) =
        (destination.strip_prefix("refs/tags/")).or_else(|| destination.strip_prefix("tags/"))
    {
        return Reach::Tags(vec![tag.to_owned()]);
    }
    if let Some(branch) =
        (destination.strip_prefix("refs/heads/")).or_else(|| destination.strip_prefix("heads/"))
    {
        return branch_reach(branch, remote, cwd);
    }
    if outside_branches_and_tags(destination) {
        return Reach::Branches;
    }
    if destination.is_empty() || destination.starts_with("refs/") {
        return Reach::Tags(Vec::new());
    }
    let tag = is_ref(&format!("refs/tags/{destination}"));
    match (tag, is_ref(&format!("refs/remotes/{remote}/{destination}"))) {
        (false, true) => branch_reach(destination, remote, cwd),
        (true, false) => Reach::Tags(vec![destination.to_owned()]),
        _ => Reach::Tags(Vec::new()),
    }
}

/// How many refs the push `what` names.
pub fn named_refs(what: &str) -> usize {
    refspecs(what).map_or(0, |(_, refspecs)| refspecs.len())
}

/// The options and refspecs of the push `what`, after its remote.
fn refspecs(what: &str) -> Option<(Vec<&str>, Vec<&str>)> {
    let mut words = what.split_whitespace().skip_while(|word| *word != "push");
    words.next()?;
    let (mut options, mut positional) = (Vec::new(), Vec::new());
    while let Some(word) = words.next() {
        match word {
            "-o" | "--push-option" | "--receive-pack" | "--exec" => {
                words.next();
            }
            word if word.starts_with('-') => options.push(word),
            word => positional.push(word),
        }
    }
    Some((options, positional.into_iter().skip(1).collect()))
}

/// The refs the push `what` deletes, as the person names them, when deleting is all it does.
pub fn deletes(what: &str) -> Option<Vec<String>> {
    let (options, refspecs) = refspecs(what).filter(|(_, refspecs)| !refspecs.is_empty())?;
    let flagged = options
        .iter()
        .any(|arg| *arg == "--delete" || (!arg.starts_with("--") && arg.contains('d')));
    let named = |name: &str| match (name.strip_prefix("refs/tags/"))
        .or_else(|| name.strip_prefix("tags/"))
    {
        Some(tag) => format!("tag {tag}"),
        None => {
            let branch = (name.strip_prefix("refs/heads/")).or_else(|| name.strip_prefix("heads/"));
            branch.map_or_else(|| name.to_owned(), |branch| format!("branch {branch}"))
        }
    };
    (refspecs.iter())
        .map(|refspec| {
            let refspec = refspec.trim_start_matches('+');
            match flagged {
                true => Some(named(refspec)),
                false => refspec
                    .strip_prefix(':')
                    .filter(|name| !name.is_empty())
                    .map(named),
            }
        })
        .collect()
}

fn resolved_or(resolved: &str, source: &str) -> String {
    match resolved.is_empty() {
        true => source.to_owned(),
        false => resolved.to_owned(),
    }
}

fn branch_reach(branch: &str, remote: &str, cwd: &Path) -> Reach {
    let default = Git::quick(cwd)
        .trimmed(&["rev-parse", "--abbrev-ref", &format!("{remote}/HEAD")])
        .ok()
        .and_then(|head| head.strip_prefix(&format!("{remote}/")).map(str::to_owned));
    let Some(default) = default else {
        return Reach::Project;
    };
    let kept = branch == default
        || KEPT_BRANCHES.contains(&branch)
        || branch.starts_with("release")
        || branch == "HEAD"
        || says_production(branch);
    match kept {
        true => Reach::Project,
        false => Reach::Branches,
    }
}

#[cfg(any(test, feature = "testing"))]
pub mod tests {
    use super::*;

    #[expect(
        clippy::unwrap_used,
        reason = "a test helper fails the test when git does"
    )]
    pub fn git(dir: &Path, args: &[&str]) {
        let ran = sikemux_process::user_environment::command("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "a")
            .env("GIT_AUTHOR_EMAIL", "a@example.com")
            .env("GIT_COMMITTER_NAME", "a")
            .env("GIT_COMMITTER_EMAIL", "a@example.com")
            .output()
            .unwrap();
        assert!(
            ran.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&ran.stderr)
        );
    }

    #[cfg(test)]
    /// A clone of a repository on github.com's spelling, on branch `feature`, with a local
    /// tag `rc` and the remote's default branch known as main.
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
        git(&work, &["commit", "--allow-empty", "-m", "first"]);
        git(
            &work,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        git(&work, &["push", "-u", "origin", "main"]);
        git(&work, &["remote", "set-head", "origin", "main"]);
        git(&work, &["checkout", "-b", "feature"]);
        git(&work, &["push", "-u", "origin", "feature"]);
        git(&work, &["tag", "rc"]);
        git(
            &work,
            &[
                "remote",
                "add",
                "production",
                "deploy@web1.example.net:apps/shop",
            ],
        );
        dir
    }

    #[cfg(test)]
    fn reached(dir: &tempfile::TempDir, line: &str) -> Reach {
        reach(line, &dir.path().join("work"))
    }

    #[test]
    fn a_branch_of_its_own_is_only_a_branch() {
        let dir = checkout();
        for line in [
            "git push origin feature",
            "git push -u origin feature",
            "git push --force-with-lease origin feature",
            "git push",
            "git push origin :feature",
            "git push -d origin feature",
            "git push --delete origin feature",
        ] {
            assert_eq!(reached(&dir, line), Reach::Branches, "{line}");
        }
    }

    /// An agent pushed its own clone of another repository with `git -C`, and was asked
    /// whether to change the project's stable releases.
    #[test]
    fn a_push_run_in_another_folder_is_read_there() {
        let dir = checkout();
        assert_eq!(
            reach("git -C work push -q origin feature", dir.path()),
            Reach::Branches
        );
    }

    /// Each spelling a reviewer found reaching the default branch, a tag or another remote
    /// while the first take on pushes let it run.
    #[test]
    fn every_spelling_that_can_reach_the_project_branch_counts_as_reaching_it() {
        let dir = checkout();
        for line in [
            "git push origin HEAD:heads/main",
            "git push origin HEAD:refs/heads/main",
            "git push origin refs/heads/*:refs/heads/*",
            "git -c remote.origin.push=HEAD:refs/heads/main push",
            "git --git-dir=x push origin feature",
            "git push origin gh-pages",
            "git push origin main",
            "git push origin release-2",
            "git push origin :main",
            "git push origin :refs/heads/main",
            "git push -d origin release-2",
        ] {
            let reached = reached(&dir, line);
            assert!(matches!(reached, Reach::Project | Reach::Tags(_)), "{line}");
        }
    }

    /// Deleting a proof branch on GitHub asked the person to publish tags to Releases.
    #[test]
    fn a_delete_reaches_what_it_deletes() {
        let dir = checkout();
        assert_eq!(
            reached(&dir, "git push -f origin :refs/heads/proof/w6-installer"),
            Reach::Branches
        );
        assert_eq!(
            reached(&dir, "git push origin :refs/tags/v1.0.0"),
            Reach::Tags(vec!["v1.0.0".to_owned()])
        );
        assert_eq!(
            reached(&dir, "git push origin --delete rc"),
            Reach::Tags(vec!["rc".to_owned()])
        );
        assert_eq!(
            deletes("git push -f origin :refs/heads/proof/w6-installer :refs/tags/v1"),
            Some(vec![
                "branch proof/w6-installer".to_owned(),
                "tag v1".to_owned()
            ])
        );
        assert_eq!(deletes("git push origin v1 :v0"), None);
    }

    #[test]
    fn on_the_default_branch_a_push_naming_no_branch_reaches_it() {
        let dir = checkout();
        git(&dir.path().join("work"), &["checkout", "main"]);
        for line in [
            "git push -o ci.skip origin",
            "git push",
            "git push origin HEAD",
        ] {
            assert_eq!(reached(&dir, line), Reach::Project, "{line}");
        }
    }

    /// A tag pushed beside the project's branch never asked, since the branch answered first.
    #[test]
    fn a_tag_pushed_with_the_branch_is_still_a_tag() {
        let dir = checkout();
        let work = dir.path().join("work");
        git(&work, &["tag", "v1.2.3"]);
        assert_eq!(
            reached(&dir, "git push origin main v1.2.3"),
            Reach::Tags(vec!["v1.2.3".to_owned()])
        );
        assert_eq!(
            reached(&dir, "git push origin main --follow-tags"),
            Reach::Tags(Vec::new())
        );
        git(&work, &["config", "remote.origin.mirror", "true"]);
        assert_eq!(
            reached(&dir, "git push origin feature"),
            Reach::Tags(Vec::new())
        );
        git(&work, &["config", "--unset", "remote.origin.mirror"]);
        assert_eq!(
            reached(
                &dir,
                "git push --recurse-submodules=on-demand origin feature"
            ),
            Reach::Tags(Vec::new())
        );
        assert_eq!(
            reached(&dir, "GIT_CONFIG_COUNT=1 git push origin feature"),
            Reach::Tags(Vec::new())
        );
    }

    #[test]
    fn a_tag_by_its_source_or_its_destination_is_a_tag() {
        let dir = checkout();
        for line in [
            "git push origin rc:v2.0.0",
            "git push origin rc",
            "git push origin HEAD:refs/tags/v2.0.0",
            "git push --tags origin",
            "git push origin feature --follow-tags",
        ] {
            assert!(matches!(reached(&dir, line), Reach::Tags(_)), "{line}");
        }
    }

    /// git takes any unique prefix of a long option, resolves a refspec's source itself, and
    /// sends annotated tags with push.followTags set.
    #[test]
    fn what_git_would_read_differently_counts_as_reaching_the_protected_refs() {
        let dir = checkout();
        for line in [
            "git push --al origin",
            "git push --all origin",
            "git push --mirr origin",
            "git push --mirror origin",
            "git push --del origin feature",
            "git push --force-with origin feature",
            "git push --tag origin",
            "git push origin tags/rc:v2.0.0",
        ] {
            assert!(matches!(reached(&dir, line), Reach::Tags(_)), "{line}");
        }
        git(
            &dir.path().join("work"),
            &["config", "push.followTags", "true"],
        );
        assert_eq!(
            reached(&dir, "git push origin feature"),
            Reach::Tags(Vec::new())
        );
    }

    #[test]
    fn what_plain_push_does_is_read_from_git() {
        let dir = checkout();
        let work = dir.path().join("work");
        git(&work, &["config", "push.default", "matching"]);
        assert_eq!(reached(&dir, "git push"), Reach::Project);
        git(&work, &["config", "--unset", "push.default"]);
        git(
            &work,
            &["config", "remote.origin.push", "HEAD:refs/heads/main"],
        );
        assert_eq!(reached(&dir, "git push"), Reach::Tags(Vec::new()));
        assert_eq!(reached(&dir, "git push origin"), Reach::Tags(Vec::new()));
    }

    #[test]
    fn with_no_default_branch_known_nothing_is_only_a_branch() {
        let dir = checkout();
        git(
            &dir.path().join("work"),
            &["remote", "set-head", "origin", "--delete"],
        );
        assert_eq!(reached(&dir, "git push origin feature"), Reach::Project);
        assert_eq!(reached(&dir, "git push origin develop"), Reach::Project);
    }

    #[test]
    fn a_push_to_a_remote_off_the_code_host_is_a_deploy() {
        let dir = checkout();
        assert_eq!(
            reached(&dir, "git push production hotfix"),
            Reach::Deploy {
                tool: "the production remote".to_owned(),
                app: "shop".to_owned()
            }
        );
        assert_eq!(
            reached(&dir, "git push https://git.heroku.com/acme-web.git main"),
            Reach::Deploy {
                tool: "Heroku".to_owned(),
                app: "acme-web".to_owned()
            }
        );
        assert!(matches!(
            reached(&dir, "git push dokku@apps.example.net:shop feature"),
            Reach::Deploy { tool, .. } if tool == "Dokku"
        ));
    }
}
