//! Where a `git push` lands, read as git resolves it: its remote, its refspecs or what
//! `push.default` and `@{push}` make of none, and whether each source and destination is a
//! branch or a tag. Whatever this can't prove stays off the project's own branch, a release
//! branch and every tag counts as reaching them.

use std::path::Path;

use super::place::says_production;
use crate::git::Git;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Reach {
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
const CODE_HOSTS: [&str; 5] = [
    "github.com",
    "gitlab.com",
    "bitbucket.org",
    "codeberg.org",
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

/// Where the push `what`, run from `cwd`, lands, judged ref by ref: any tag it carries is
/// judged as a tag pushed alone would be.
pub(crate) fn reach(what: &str, cwd: &Path) -> Reach {
    let unknown = || Reach::Tags(Vec::new());
    let words: Vec<&str> = what.split_whitespace().collect();
    let Some(("git", rest)) = words.split_first().map(|(git, rest)| (*git, rest)) else {
        return unknown();
    };
    let Some(("push", args)) = rest.split_first().map(|(push, args)| (*push, args)) else {
        return unknown();
    };
    let git = || Git::quick(cwd);
    let mut positional: Vec<&str> = Vec::new();
    let mut tags = false;
    let mut at = 0;
    while let Some(arg) = args.get(at) {
        at += 1;
        match *arg {
            "-o" | "--push-option" | "--receive-pack" | "--exec" => at += 1,
            "--tags" | "--follow-tags" => tags = true,
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
                let letters = &arg[1..];
                if letters.starts_with('o') {
                    continue;
                }
                if !letters.chars().all(|letter| "funqv46".contains(letter)) {
                    return unknown();
                }
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
            .map(|refspec| refspec_reach(refspec, &remote, cwd))
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
    if source.is_empty() {
        return Reach::Tags(Vec::new());
    }
    let git = || Git::quick(cwd);
    let is_ref = |full: &str| {
        git()
            .trimmed(&["show-ref", "--verify", "--quiet", full])
            .is_ok()
    };
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

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn git(dir: &Path, args: &[&str]) {
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
        ] {
            assert_eq!(reached(&dir, line), Reach::Branches, "{line}");
        }
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
            "git push origin :feature",
            "git push origin gh-pages",
            "git push origin main",
            "git push origin release-2",
            "git push -d origin feature",
        ] {
            let reached = reached(&dir, line);
            assert!(matches!(reached, Reach::Project | Reach::Tags(_)), "{line}");
        }
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
