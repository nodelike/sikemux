//! Where a `git push` lands, read as git resolves it: its remote, its refspecs or what
//! `push.default` and the branch's config make of none, and whether each source and destination is a
//! branch or a tag. Whatever this can't prove stays off the project's own branch, a release
//! branch and every tag counts as reaching them.

use std::path::{Path, PathBuf};

use super::place::says_production;
use crate::git::Git;
use crate::ship::Lands;

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

/// Options of `git push` that leave it one that only sends commits forward.
const PLAIN_OPTIONS: [&str; 12] = [
    "-u",
    "--set-upstream",
    "-q",
    "--quiet",
    "-v",
    "--verbose",
    "--verify",
    "--porcelain",
    "--progress",
    "--no-progress",
    "--atomic",
    "--no-follow-tags",
];

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

/// The words of a push that only sends commits forward: nothing forced or deleted, no hook
/// skipped, and its one branch or tag sent to the remote the project's code is on.
struct PlainWords<'a> {
    /// What it names to send; none for the checkout's branch.
    refspec: Option<&'a str>,
    follows_tags: bool,
}

fn plain_words(what: &str) -> Option<PlainWords<'_>> {
    let words: Vec<&str> = what.split_whitespace().collect();
    let args = match words.as_slice() {
        ["git", "push", args @ ..] | ["git", "-C", _, "push", args @ ..] => args,
        _ => return None,
    };
    let (options, named): (Vec<&str>, Vec<&str>) =
        args.iter().copied().partition(|arg| arg.starts_with('-'));
    let refspec = match named.as_slice() {
        [] | ["origin"] => None,
        ["origin", refspec] => Some(*refspec),
        _ => return None,
    };
    let forward =
        refspec.is_none_or(|refspec| !refspec.starts_with(['+', ':']) && !refspec.contains('*'));
    (forward && options.iter().all(|option| PLAIN_OPTIONS.contains(option))).then(|| PlainWords {
        refspec,
        follows_tags: !options.contains(&"--no-follow-tags"),
    })
}

/// What git's config says of `key` in the checkout at `cwd`.
fn config(key: &str, cwd: &Path) -> Option<String> {
    let said = Git::quick(cwd)
        .trimmed(&["config", "--get-all", key])
        .ok()?;
    let last = said.lines().last()?;
    Some(last.to_owned())
}

fn config_on(key: &str, cwd: &Path) -> bool {
    (Git::quick(cwd).trimmed(&["config", "--bool", key])).is_ok_and(|said| said == "true")
}

/// Whether git's config in the checkout at `cwd` makes a push to `remote` send other refs than
/// its words name.
fn config_redirects(remote: &str, follows_tags: bool, cwd: &Path) -> bool {
    let submodules = config("push.recurseSubmodules", cwd)
        .or_else(|| config("submodule.recurse", cwd))
        .is_some_and(|said| !["no", "false", "check"].contains(&said.as_str()));
    config(&format!("remote.{remote}.push"), cwd).is_some()
        || pushes_elsewhere(remote, cwd)
        || config_on(&format!("remote.{remote}.mirror"), cwd)
        || (follows_tags && config_on("push.followTags", cwd))
        || config("push.default", cwd)
            .is_some_and(|said| !["simple", "current", "nothing"].contains(&said.as_str()))
        || submodules
}

/// Whether `remote` is pushed to another repository than it is fetched from.
fn pushes_elsewhere(remote: &str, cwd: &Path) -> bool {
    let repository = |options: &[&str]| {
        let args = [&["remote", "get-url"], options, &[remote]].concat();
        let url = Git::quick(cwd).trimmed(&args).ok()?;
        Some(match crate::server::credential::owner_and_name(&url) {
            Some((owner, name)) => format!("{owner}/{name}"),
            None => url,
        })
    };
    repository(&["--push", "--all"]) != repository(&[])
}

/// `url` as the repository it names, the same over SSH and HTTPS, without who signs in to it.
fn repository_at(url: &str) -> String {
    let url = url.trim_end_matches('/');
    let url = url.strip_suffix(".git").unwrap_or(url);
    let without_user = |rest: &str| {
        let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
        let host = host.rsplit('@').next().unwrap_or(host).to_ascii_lowercase();
        match host.as_str() {
            "github.com" => format!("{host}/{}", path.to_ascii_lowercase()),
            _ => format!("{host}/{path}"),
        }
    };
    match url.split_once("://") {
        Some(("https" | "ssh", rest)) => without_user(rest),
        Some((scheme, rest)) => format!("{scheme}://{}", without_user(rest)),
        None => match url.split_once(':') {
            Some((host, path)) if !host.contains('/') && !path.starts_with('/') => {
                without_user(&format!("{host}/{path}"))
            }
            _ => url.to_owned(),
        },
    }
}

/// The one repository a push to `remote` from the checkout at `cwd` goes to, as git has it now.
fn pushed_repository(remote: &str, cwd: &Path) -> Option<String> {
    let urls = Git::quick(cwd)
        .trimmed(&["remote", "get-url", "--push", "--all", remote])
        .ok()?;
    let mut urls = urls.lines();
    let only = urls.next().filter(|_| urls.next().is_none())?;
    Some(repository_at(only))
}

/// The repository the push `what`, run from `cwd`, sends to, where it only sends the checkout's
/// own commits to the project's own branch.
pub fn plain(what: &str, cwd: &Path) -> Option<String> {
    let moved = &run_in(what, cwd);
    let repository = pushed_repository("origin", moved)?;
    is_plain(what, cwd).then_some(repository)
}

fn is_plain(what: &str, cwd: &Path) -> bool {
    let Some(words) = plain_words(what) else {
        return false;
    };
    let cwd = &run_in(what, cwd);
    if config_redirects("origin", words.follows_tags, cwd) || reach(what, cwd) != Reach::Project {
        return false;
    }
    let git = || Git::quick(cwd);
    let commit = |name: &str| {
        let name = format!("{name}^{{commit}}");
        git()
            .trimmed(&["rev-parse", "--verify", "--quiet", &name])
            .ok()
    };
    let (source, destination) = match words.refspec {
        None => {
            if push_remote(cwd).as_deref() != Some("origin") {
                return false;
            }
            ("HEAD", pushed_to(cwd))
        }
        Some(refspec) => match refspec.split_once(':') {
            Some((source, destination)) => (source, Some(destination.to_owned())),
            None if matches!(refspec, "HEAD" | "@") => (
                "HEAD",
                git().trimmed(&["symbolic-ref", "--short", "HEAD"]).ok(),
            ),
            None => (refspec, Some(refspec.to_owned())),
        },
    };
    destination.is_some_and(|destination| is_own_branch(&destination, cwd))
        && commit(source).is_some_and(|sent| Some(sent) == commit("HEAD"))
}

/// The repository the push `what`, run from `cwd`, sends to, where it sends the one tag it
/// names and nothing else.
pub fn one_tag(what: &str, cwd: &Path) -> Option<String> {
    let words = plain_words(what)?;
    let cwd = &run_in(what, cwd);
    let named = words.refspec.is_some_and(|refspec| !refspec.contains(':'));
    (named && !config_redirects("origin", words.follows_tags, cwd))
        .then(|| pushed_repository("origin", cwd))
        .flatten()
}

/// Whether `branch` is the one the project's own work lands on.
fn is_own_branch(branch: &str, cwd: &Path) -> bool {
    let branch = branch.strip_prefix("refs/heads/").unwrap_or(branch);
    ["main", "master"].contains(&branch)
        && default_branch("origin", cwd).is_none_or(|default| default == branch)
}

/// The commits the push `what`, run by git in `cwd`, sends as the checkout has them now, or
/// every branch and tag where it doesn't name them.
pub fn sent(what: &str, cwd: &Path) -> String {
    let git = || Git::quick(cwd);
    let commit = |name: &str| {
        git()
            .trimmed(&["rev-parse", "--verify", "--quiet", name])
            .unwrap_or_default()
    };
    let (options, refspecs) = refspecs(what).unwrap_or_default();
    let many = ["--all", "--branches", "--mirror", "--tags", "--follow-tags"];
    let unnamed = options.iter().any(|option| many.contains(option))
        || refspecs.iter().any(|refspec| refspec.contains('*'));
    match refspecs.is_empty() {
        _ if unnamed || config_redirects("origin", true, cwd) => git()
            .trimmed(&["for-each-ref", "refs/heads", "refs/tags"])
            .unwrap_or_default(),
        true => commit("HEAD"),
        false => {
            let sent: Vec<String> = (refspecs.iter())
                .map(|refspec| refspec.trim_start_matches('+'))
                .map(|refspec| commit(refspec.split(':').next().unwrap_or_default()))
                .collect();
            sent.join(" ")
        }
    }
}

/// Where the push `what`, run from `cwd`, lands, judged ref by ref: any tag it carries is
/// judged as a tag pushed alone would be.
pub fn reach(what: &str, cwd: &Path) -> Reach {
    let (reaches, mut unnamed) = match reaches(what, cwd) {
        Ok(read) => read,
        Err(reach) => return reach,
    };
    let mut carries_tags = unnamed;
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

/// Whether the push `what`, run from `cwd`, may reach the project's branch, whatever tags it
/// carries.
pub fn may_reach_project(what: &str, cwd: &Path) -> bool {
    reaches(what, cwd).map_or(true, |(reaches, _)| {
        reaches.iter().any(|reached| match reached {
            Reach::Project | Reach::Deploy { .. } => true,
            Reach::Tags(names) => names.is_empty(),
            Reach::Branches => false,
        })
    })
}

/// Where each ref of the push `what` lands, and whether it carries tags it doesn't name, or
/// where all of it lands when its refs can't be told apart.
fn reaches(what: &str, cwd: &Path) -> Result<(Vec<Reach>, bool), Reach> {
    let unknown = || Err(Reach::Tags(Vec::new()));
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
        return Err(deploy);
    }
    if config_on(&format!("remote.{remote}.mirror"), cwd) {
        return unknown();
    }
    let reaches = match refspecs.is_empty() {
        true => {
            let default = config("push.default", cwd).unwrap_or_default();
            if config(&format!("remote.{remote}.push"), cwd).is_some() {
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
    Ok((reaches, tags || config_on("push.followTags", cwd)))
}

/// The pull request a `gh pr merge` merges, as its words name it.
pub struct Merging {
    /// Its number, address or branch; none for the current branch's.
    pull: Option<String>,
    /// The `owner/name` of its repository, when the words name one.
    pub repo: Option<String>,
    /// Whether every word was understood, so that `pull` is the one merged.
    read: bool,
    /// Whether it goes past what the repository asks of a merge.
    overrides: bool,
    /// Whether it deletes the pull request's branch.
    deletes: bool,
    /// Whether GitHub merges it later, once its checks pass, rather than now.
    pub later: bool,
}

/// The merge GitHub's API makes of `pull` in `repo`.
pub fn api_merging(pull: &str, repo: &str) -> Merging {
    Merging {
        pull: Some(pull.to_owned()),
        repo: (!repo.contains('{')).then(|| repo.to_owned()),
        read: true,
        overrides: true,
        deletes: false,
        later: false,
    }
}

pub fn merging(what: &str) -> Merging {
    let mut found = Merging {
        pull: None,
        repo: None,
        read: false,
        overrides: false,
        deletes: false,
        later: false,
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
            "--auto" => found.later = true,
            "-m" | "--merge" | "-s" | "--squash" | "-r" | "--rebase" => {}
            "-d" | "--delete-branch" => found.deletes = true,
            "--admin" => found.overrides = true,
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

pub struct MergeReach {
    pub reach: Reach,
    pub head: Option<String>,
    pub repository: Option<String>,
    pub branch: Option<String>,
    pub files: Option<Vec<String>>,
}

impl Merging {
    /// Whether the pull request is of the repository checked out at `root`.
    pub fn is_into(&self, root: &Path) -> bool {
        (self.repo.as_ref()).is_none_or(|repo| is_remote_of(repo, root))
    }

    /// Where a merge run from `cwd` lands in the repository checked out at `root`, and the
    /// commit it merges, and the repository it does so in where it only goes into the project's
    /// own branch, as the repository asks. GitHub is asked for the branch it goes into.
    pub fn reach(&self, root: &Path, cwd: &Path) -> MergeReach {
        if !self.read {
            return MergeReach {
                reach: Reach::Project,
                head: None,
                repository: None,
                branch: None,
                files: None,
            };
        }
        let mut view = sikemux_process::user_environment::command("gh");
        view.current_dir(cwd)
            .env("GH_PROMPT_DISABLED", "1")
            .args(["pr", "view"])
            .args(&self.pull)
            .args(["--json", "baseRefName,headRefName,headRefOid,url,files"]);
        if let Some(repo) = &self.repo {
            view.args(["--repo", repo]);
        }
        let found = (view.output().ok())
            .filter(|output| output.status.success())
            .and_then(|output| serde_json::from_slice::<serde_json::Value>(&output.stdout).ok());
        let field = |name: &str| found.as_ref()?.get(name)?.as_str().map(str::to_owned);
        let branch = field("baseRefName");
        let reach = branch
            .as_deref()
            .map_or(Reach::Project, |base| branch_reach(base, "origin", root));
        let own = branch
            .as_deref()
            .is_some_and(|base| is_own_branch(base, root));
        let work = |head: String| branch_reach(&head, "origin", root) == Reach::Branches;
        let keeps = !self.deletes || field("headRefName").is_some_and(work);
        let repository = field("url")
            .and_then(|url| Some(repository_at(url.split_once("/pull/")?.0)))
            .filter(|_| own && !self.overrides && keeps);
        let files = found
            .as_ref()
            .and_then(|value| value.get("files")?.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.get("path")?.as_str().map(str::to_owned))
                    .collect()
            });
        MergeReach {
            reach,
            head: field("headRefOid"),
            repository,
            branch,
            files,
        }
    }
}

/// The target branch and changed files of the git push `what` run from `cwd`, when readable.
pub fn pushed_branch_and_files(what: &str, cwd: &Path) -> (Option<String>, Option<Vec<String>>) {
    let cwd = &run_in(what, cwd);
    let Some((options, positional)) = push_words(what) else {
        return (None, None);
    };
    let many = ["--all", "--branches", "--mirror", "--tags"];
    if options.iter().any(|option| many.contains(option)) {
        return (None, None);
    }
    let (remote, refspecs) = match positional.split_first() {
        Some((remote, refspecs)) => ((*remote).to_owned(), refspecs.to_vec()),
        None => match push_remote(cwd) {
            Some(remote) => (remote, Vec::new()),
            None => return (None, None),
        },
    };
    let (source, branch) = match refspecs.as_slice() {
        [] => ("HEAD".to_owned(), pushed_to(cwd)),
        [spec] => {
            let spec = spec.trim_start_matches('+');
            if spec.contains('*') {
                return (None, None);
            }
            match spec.split_once(':') {
                Some(("", _)) => return (None, None),
                Some((src, dst)) => {
                    let dst = dst
                        .strip_prefix("refs/heads/")
                        .or_else(|| dst.strip_prefix("heads/"))
                        .unwrap_or(dst);
                    (src.to_owned(), Some(dst.to_owned()))
                }
                None if matches!(spec, "HEAD" | "@") => ("HEAD".to_owned(), pushed_to(cwd)),
                None => {
                    let branch = spec
                        .strip_prefix("refs/heads/")
                        .or_else(|| spec.strip_prefix("heads/"))
                        .unwrap_or(spec);
                    (spec.to_owned(), Some(branch.to_owned()))
                }
            }
        }
        _ => return (None, None),
    };
    let Some(branch) = branch else {
        return (None, None);
    };
    let base = format!("refs/remotes/{remote}/{branch}");
    let git = Git::quick(cwd);
    let files = git
        .trimmed(&["diff", "--name-only", &format!("{base}..{source}")])
        .ok()
        .map(|out| {
            out.lines()
                .filter(|line| !line.is_empty())
                .map(str::to_owned)
                .collect()
        });
    (Some(branch), files)
}

/// Whether `repo`, an `owner/name` or address, is a remote of the checkout at `root`.
pub fn is_remote_of(repo: &str, root: &Path) -> bool {
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
            (theirs.eq_ignore_ascii_case(owner) || owner == "{owner}")
                && (its.eq_ignore_ascii_case(name) || name == "{repo}")
        })
}

/// Where a write through GitHub's API to the code of the repository checked out at `root` lands.
pub fn api_reach(lands: &Lands, root: &Path) -> Reach {
    match lands {
        Lands::Tag { name } | Lands::Release { tag: name } => Reach::Tags(vec![name.clone()]),
        Lands::Branch { name } => {
            let has = |branch: &str| {
                let full = format!("refs/remotes/origin/{branch}");
                (Git::quick(root).trimmed(&["show-ref", "--verify", "--quiet", &full])).is_ok()
            };
            let default = (default_branch("origin", root)).or_else(|| {
                ["main", "master"]
                    .into_iter()
                    .find(|branch| has(branch))
                    .map(Into::into)
            });
            reach_beside(name, default)
        }
        Lands::Any | Lands::Unread => Reach::Tags(Vec::new()),
        Lands::Merge { .. } | Lands::AutoMerge => Reach::Project,
    }
}

/// The remote a push with none named goes to, as git picks it from its config.
fn push_remote(cwd: &Path) -> Option<String> {
    let branch = current_branch(cwd)?;
    (config(&format!("branch.{branch}.pushRemote"), cwd))
        .or_else(|| config("remote.pushDefault", cwd))
        .or_else(|| fetched_from(&branch, cwd))
        .or_else(|| Some("origin".to_owned()))
}

/// The branch a push with no refspec updates, as `push.default` picks it. None where git would
/// refuse the push or send it outside the remote's branches.
fn pushed_to(cwd: &Path) -> Option<String> {
    let branch = current_branch(cwd)?;
    let merged = || {
        let merge = config(&format!("branch.{branch}.merge"), cwd)?;
        merge.strip_prefix("refs/heads/").map(str::to_owned)
    };
    let own_remote = || {
        let fetched = fetched_from(&branch, cwd).unwrap_or_else(|| "origin".to_owned());
        push_remote(cwd).is_some_and(|remote| remote == fetched)
    };
    match config("push.default", cwd).as_deref().unwrap_or("simple") {
        "current" => Some(branch),
        "upstream" | "tracking" => merged().filter(|_| own_remote()),
        "simple" if own_remote() => match merged() {
            Some(merged) => (merged == branch).then_some(branch),
            None => config_on("push.autoSetupRemote", cwd).then_some(branch),
        },
        "simple" => Some(branch),
        _ => None,
    }
}

fn current_branch(cwd: &Path) -> Option<String> {
    Git::quick(cwd)
        .trimmed(&["symbolic-ref", "--short", "HEAD"])
        .ok()
}

fn fetched_from(branch: &str, cwd: &Path) -> Option<String> {
    config(&format!("branch.{branch}.remote"), cwd)
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
        None if config(&format!("remote.{remote}.push"), cwd).is_some() => {
            return Reach::Tags(Vec::new());
        }
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
    let upstream = (destination.is_none())
        .then(|| upstream_of(&source, cwd))
        .flatten();
    let destination = destination.or(upstream.as_deref()).unwrap_or(&source);
    if destination.is_empty() {
        return Reach::Project;
    }
    if destination.starts_with("refs/tags/") || destination.starts_with("tags/") {
        return Reach::Tags(vec![short(destination)]);
    }
    let version = destination
        .strip_prefix('v')
        .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()));
    if refspec.contains(':') && version && !is_ref(&format!("refs/remotes/{remote}/{destination}"))
    {
        return Reach::Tags(vec![destination.to_owned()]);
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

/// The branch `push.default` sends the branch `source` to, when not one of its own name.
fn upstream_of(source: &str, cwd: &Path) -> Option<String> {
    config("push.default", cwd).filter(|said| said == "upstream" || said == "tracking")?;
    let source = (source.strip_prefix("refs/heads/"))
        .or_else(|| source.strip_prefix("heads/"))
        .unwrap_or(source);
    config(&format!("branch.{source}.merge"), cwd)
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
    let (options, positional) = push_words(what)?;
    Some((options, positional.into_iter().skip(1).collect()))
}

/// The options of the push `what`, and the words it names: its remote, then its refspecs.
fn push_words(what: &str) -> Option<(Vec<&str>, Vec<&str>)> {
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
    Some((options, positional))
}

/// The repositories the push `what`, run by git in `cwd`, sends to as git has its remote now.
pub fn destination(what: &str, cwd: &Path) -> String {
    let named =
        push_words(what).and_then(|(_, named)| named.first().map(|remote| (*remote).to_owned()));
    let Some(remote) = named.or_else(|| push_remote(cwd)) else {
        return String::new();
    };
    match Git::quick(cwd).trimmed(&["remote", "get-url", "--push", "--all", &remote]) {
        Ok(urls) => urls
            .lines()
            .map(repository_at)
            .collect::<Vec<_>>()
            .join(" "),
        Err(_) => repository_at(&remote),
    }
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

/// The branch `remote` says is its own, as the checkout at `cwd` last saw it.
fn default_branch(remote: &str, cwd: &Path) -> Option<String> {
    let head = Git::quick(cwd)
        .trimmed(&["rev-parse", "--abbrev-ref", &format!("{remote}/HEAD")])
        .ok()?;
    head.strip_prefix(&format!("{remote}/")).map(str::to_owned)
}

fn branch_reach(branch: &str, remote: &str, cwd: &Path) -> Reach {
    reach_beside(branch, default_branch(remote, cwd))
}

/// What `branch` is in a repository whose own branch is `default`.
fn reach_beside(branch: &str, default: Option<String>) -> Reach {
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
    fn a_first_push_lands_on_a_branch_of_its_own() {
        let dir = checkout();
        let work = dir.path().join("work");
        git(&work, &["remote", "remove", "production"]);
        git(&work, &["checkout", "-b", "fresh"]);
        git(&work, &["config", "push.autoSetupRemote", "false"]);
        git(&work, &["config", "push.default", "simple"]);
        assert_eq!(reached(&dir, "git push"), Reach::Tags(Vec::new()));
        git(&work, &["config", "push.autoSetupRemote", "true"]);
        assert_eq!(reached(&dir, "git push"), Reach::Branches);
        assert_eq!(
            reached(&dir, "git push -f origin HEAD:v0.7.0"),
            Reach::Tags(vec!["v0.7.0".to_owned()])
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
