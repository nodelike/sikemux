//! Actions an agent takes that leave this machine: pushing commits, merging a
//! pull request, deploying, publishing, writing to a production database, and
//! writing to a website or a connector.
//! The one table of which commands and Sikemux tools they are, read by the
//! shim that stands in for those programs, by the hook that checks a shell
//! command before it runs, and by the core for its own tools.

use std::path::Path;

use serde::{Deserialize, Serialize};
use sikemux_view::inbox::ShipKind;

use crate::git_guard::{base_name, command_words, git_args, segments, shell_script};

/// An action leaving this machine: what it does, and the exact command or
/// call, which the person is shown and allowing lets run once.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ship {
    pub kind: ShipKind,
    pub what: String,
    /// Where it lands, when hard facts say; none counts as production.
    pub target: Option<Target>,
}

/// Where an action lands, read from what it really runs with, never a guess.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Target {
    /// A Rundeck job by its group/name path; `production` when Rundeck's own
    /// settings say its folder or project is.
    RundeckJob {
        path: String,
        production: bool,
    },
    Vercel {
        production: bool,
    },
    AwsProfile {
        name: String,
    },
    KubeContext {
        name: String,
    },
    TerraformWorkspace {
        name: String,
    },
    /// A website, by its origin, such as `https://github.com`.
    Site {
        origin: String,
    },
    /// A connector's service, such as Jira.
    Connector {
        name: String,
    },
}

impl Target {
    /// It is production whatever the project calls it.
    pub(crate) fn production(&self) -> bool {
        match self {
            Self::RundeckJob { production, .. } | Self::Vercel { production } => *production,
            Self::AwsProfile { .. }
            | Self::KubeContext { .. }
            | Self::TerraformWorkspace { .. }
            | Self::Site { .. }
            | Self::Connector { .. } => false,
        }
    }
}

impl Ship {
    /// The site or connector it writes to, as the person reads it.
    pub(crate) fn writes_to(&self) -> Option<&str> {
        match &self.target {
            Some(Target::Site { origin }) => Some(origin),
            Some(Target::Connector { name }) => Some(name),
            _ => None,
        }
    }

    /// The words an always-allow of what it writes to is kept under.
    pub(crate) fn written_place(&self) -> Option<Vec<String>> {
        let (word, place) = match &self.target {
            Some(Target::Site { origin }) => ("site", origin),
            Some(Target::Connector { name }) => ("connector", name),
            _ => return None,
        };
        Some(vec![word.to_owned(), place.clone()])
    }
}

/// Whether a host is this computer's own, which a write to never leaves it.
fn local_host(host: &str) -> bool {
    host == "localhost" || host == "[::1]" || host.ends_with(".local") || host.starts_with("127.")
}

/// What `curl` or `wget` is pointed at: each address it was given, or its last word when
/// none has a scheme, which it reads as a host.
fn web_addresses(args: &[String]) -> Vec<url::Url> {
    let schemed = |arg: &&String| ["http://", "https://"].iter().any(|at| arg.starts_with(at));
    let given: Vec<&String> = args.iter().filter(schemed).collect();
    let words = match given.is_empty() {
        false => given,
        true => (args.iter().rfind(|arg| !arg.starts_with('-')))
            .into_iter()
            .collect(),
    };
    words
        .into_iter()
        .filter_map(|word| match word.contains("://") {
            true => url::Url::parse(word).ok(),
            false => url::Url::parse(&format!("https://{word}")).ok(),
        })
        .collect()
}

/// Whether `program`, curl or wget run with `args`, sends something to a host that
/// isn't this computer. Best effort: shell words can always be arranged to get past this.
fn web_write(program: &str, args: &[String]) -> bool {
    let (data_flags, method_flags): (&[&str], &[&str]) = match program {
        "wget" => (WGET_DATA, &["--method"]),
        _ => (CURL_DATA, &["-X", "--request"]),
    };
    let attached =
        (args.iter()).find_map(|arg| arg.strip_prefix("-X").filter(|_| program == "curl"));
    let method = flag_value(args, method_flags).or(attached.map(str::to_owned));
    let names_write = method.is_some_and(|verb| {
        !matches!(
            verb.to_ascii_uppercase().as_str(),
            "GET" | "HEAD" | "OPTIONS"
        )
    });
    let packed = program == "curl"
        && args.iter().any(|arg| {
            !arg.starts_with("--") && arg.starts_with('-') && arg.contains(['d', 'F', 'T'])
        });
    let as_query = program == "curl" && has_flag(args, &["-G", "--get"]);
    if !names_write && (as_query || !(has_flag(args, data_flags) || packed)) {
        return false;
    }
    let addresses = web_addresses(args);
    addresses.is_empty()
        || addresses
            .iter()
            .any(|address| !address.host_str().is_some_and(local_host))
}

const CURL_DATA: &[&str] = &[
    "-d",
    "--data",
    "--data-raw",
    "--data-binary",
    "--data-urlencode",
    "--data-ascii",
    "--json",
    "-F",
    "--form",
    "--form-string",
    "-T",
    "--upload-file",
];
const WGET_DATA: &[&str] = &["--post-data", "--post-file", "--body-data", "--body-file"];

/// The value of a flag among `args`, as `--flag value` or `--flag=value`. None
/// when it is not given or given twice with different values.
fn flag_value(args: &[String], names: &[&str]) -> Option<String> {
    let mut found: Option<String> = None;
    for (at, arg) in args.iter().enumerate() {
        let value = names.iter().find_map(|name| {
            if arg == name {
                return args.get(at + 1).cloned();
            }
            arg.strip_prefix(name)?.strip_prefix('=').map(str::to_owned)
        });
        match (&found, value) {
            (Some(kept), Some(value)) if *kept != value => return None,
            (_, Some(value)) => found = Some(value),
            _ => {}
        }
    }
    found
}

fn has_flag(args: &[String], names: &[&str]) -> bool {
    args.iter().any(|arg| {
        names.iter().any(|name| {
            arg == name
                || arg
                    .strip_prefix(name)
                    .is_some_and(|rest| rest.starts_with('='))
        })
    })
}

/// The current context of the only kube config the program would read.
fn current_kube_context(env: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    let file = match env("KUBECONFIG") {
        Some(listed) if listed.contains(':') => return None,
        Some(file) => std::path::PathBuf::from(file),
        None => Path::new(&env("HOME")?).join(".kube/config"),
    };
    let config = std::fs::read_to_string(file).ok()?;
    config.lines().find_map(|line| {
        let name = line
            .strip_prefix("current-context:")?
            .trim()
            .trim_matches(['"', '\'']);
        (!name.is_empty()).then(|| name.to_owned())
    })
}

/// Where `program`, run with `args` from `cwd` under `env`, lands. None when
/// its own flags or environment leave that open, or could point it elsewhere,
/// as another config or credentials given beside the name.
pub(crate) fn target_of(
    program: &str,
    args: &[String],
    env: &dyn Fn(&str) -> Option<String>,
    cwd: &Path,
) -> Option<Target> {
    match base_name(program) {
        "vercel" => has_flag(args, &["--prod"])
            .then_some(())
            .or_else(|| args.iter().any(|arg| arg == "promote").then_some(()))
            .map(|()| Target::Vercel { production: true }),
        "aws" => {
            let name = match flag_value(args, &["--profile"]) {
                Some(name) => name,
                None if env("AWS_ACCESS_KEY_ID").is_some() => return None,
                None => env("AWS_PROFILE")?,
            };
            Some(Target::AwsProfile { name })
        }
        "kubectl" | "helm" => {
            let elsewhere = [
                "--kubeconfig",
                "--server",
                "-s",
                "--cluster",
                "--user",
                "--token",
                "--kube-apiserver",
                "--kube-token",
            ];
            if has_flag(args, &elsewhere) {
                return None;
            }
            let name = flag_value(args, &["--context", "--kube-context"])
                .or_else(|| current_kube_context(env))?;
            Some(Target::KubeContext { name })
        }
        "curl" | "wget" => (web_addresses(args).into_iter())
            .find(|address| !address.host_str().is_some_and(local_host))
            .map(|address| Target::Site {
                origin: address.origin().ascii_serialization(),
            }),
        "terraform" => {
            let dir =
                flag_value(args, &["-chdir"]).map_or_else(|| cwd.to_owned(), |dir| cwd.join(dir));
            let name = env("TF_WORKSPACE").or_else(|| {
                std::fs::read_to_string(dir.join(".terraform/environment"))
                    .ok()
                    .map(|name| name.trim().to_owned())
            })?;
            Some(Target::TerraformWorkspace { name })
        }
        _ => None,
    }
}

/// The programs besides git the shim stands in for, so their actions that
/// leave this machine are checked before they run.
pub(crate) const PROGRAMS: &[&str] = &[
    "gh",
    "glab",
    "vercel",
    "netlify",
    "fly",
    "flyctl",
    "firebase",
    "wrangler",
    "railway",
    "kubectl",
    "helm",
    "terraform",
    "pulumi",
    "serverless",
    "sls",
    "aws",
    "eas",
    "docker",
    "npm",
    "pnpm",
    "yarn",
    "cargo",
    "curl",
    "wget",
];

/// What a rule's words need besides themselves to take the action.
enum Needs {
    Nothing,
    /// The command also passes this flag.
    Flag(&'static str),
    /// The command's last word names an S3 bucket, so it writes there.
    ToS3,
}

/// A program's action that leaves this machine: `words` follow one another
/// among its arguments.
struct Rule {
    programs: &'static [&'static str],
    words: &'static [&'static str],
    needs: Needs,
    kind: ShipKind,
}

const fn rule(
    programs: &'static [&'static str],
    words: &'static [&'static str],
    needs: Needs,
    kind: ShipKind,
) -> Rule {
    Rule {
        programs,
        words,
        needs,
        kind,
    }
}

const KUBECTL: &[&str] = &["kubectl"];
const HELM: &[&str] = &["helm"];
const AWS: &[&str] = &["aws"];
const NODE_PACKAGES: &[&str] = &["npm", "pnpm", "yarn"];

const RULES: &[Rule] = &[
    rule(&["gh"], &["pr", "merge"], Needs::Nothing, ShipKind::Merge),
    rule(&["glab"], &["mr", "merge"], Needs::Nothing, ShipKind::Merge),
    rule(
        &["gh"],
        &["workflow", "run"],
        Needs::Nothing,
        ShipKind::Deploy,
    ),
    rule(
        &["gh"],
        &["release", "create"],
        Needs::Nothing,
        ShipKind::Publish,
    ),
    rule(&["vercel"], &[], Needs::Flag("--prod"), ShipKind::Deploy),
    rule(&["vercel"], &["promote"], Needs::Nothing, ShipKind::Deploy),
    rule(&["vercel"], &["redeploy"], Needs::Nothing, ShipKind::Deploy),
    rule(&["vercel"], &["alias"], Needs::Nothing, ShipKind::Deploy),
    rule(
        &["netlify"],
        &["deploy"],
        Needs::Flag("--prod"),
        ShipKind::Deploy,
    ),
    rule(
        &["fly", "flyctl"],
        &["deploy"],
        Needs::Nothing,
        ShipKind::Deploy,
    ),
    rule(&["firebase"], &["deploy"], Needs::Nothing, ShipKind::Deploy),
    rule(&["wrangler"], &["deploy"], Needs::Nothing, ShipKind::Deploy),
    rule(
        &["wrangler"],
        &["publish"],
        Needs::Nothing,
        ShipKind::Deploy,
    ),
    rule(&["railway"], &["up"], Needs::Nothing, ShipKind::Deploy),
    rule(KUBECTL, &["apply"], Needs::Nothing, ShipKind::Deploy),
    rule(KUBECTL, &["rollout"], Needs::Nothing, ShipKind::Deploy),
    rule(KUBECTL, &["set"], Needs::Nothing, ShipKind::Deploy),
    rule(KUBECTL, &["delete"], Needs::Nothing, ShipKind::Deploy),
    rule(KUBECTL, &["scale"], Needs::Nothing, ShipKind::Deploy),
    rule(KUBECTL, &["patch"], Needs::Nothing, ShipKind::Deploy),
    rule(HELM, &["install"], Needs::Nothing, ShipKind::Deploy),
    rule(HELM, &["upgrade"], Needs::Nothing, ShipKind::Deploy),
    rule(HELM, &["uninstall"], Needs::Nothing, ShipKind::Deploy),
    rule(HELM, &["rollback"], Needs::Nothing, ShipKind::Deploy),
    rule(&["terraform"], &["apply"], Needs::Nothing, ShipKind::Deploy),
    rule(
        &["terraform"],
        &["destroy"],
        Needs::Nothing,
        ShipKind::Deploy,
    ),
    rule(&["pulumi"], &["up"], Needs::Nothing, ShipKind::Deploy),
    rule(&["pulumi"], &["destroy"], Needs::Nothing, ShipKind::Deploy),
    rule(
        &["serverless", "sls"],
        &["deploy"],
        Needs::Nothing,
        ShipKind::Deploy,
    ),
    rule(
        AWS,
        &["cloudformation", "deploy"],
        Needs::Nothing,
        ShipKind::Deploy,
    ),
    rule(
        AWS,
        &["ecs", "update-service"],
        Needs::Nothing,
        ShipKind::Deploy,
    ),
    rule(
        AWS,
        &["lambda", "update-function-code"],
        Needs::Nothing,
        ShipKind::Deploy,
    ),
    rule(AWS, &["s3", "cp"], Needs::ToS3, ShipKind::Deploy),
    rule(AWS, &["s3", "sync"], Needs::ToS3, ShipKind::Deploy),
    rule(AWS, &["s3", "mv"], Needs::ToS3, ShipKind::Deploy),
    rule(&["eas"], &["update"], Needs::Nothing, ShipKind::Deploy),
    rule(&["eas"], &["submit"], Needs::Nothing, ShipKind::Publish),
    rule(&["docker"], &["push"], Needs::Nothing, ShipKind::Publish),
    rule(
        &["docker"],
        &["buildx", "build"],
        Needs::Flag("--push"),
        ShipKind::Publish,
    ),
    rule(
        NODE_PACKAGES,
        &["publish"],
        Needs::Nothing,
        ShipKind::Publish,
    ),
    rule(&["cargo"], &["publish"], Needs::Nothing, ShipKind::Publish),
];

/// What `program`, run with `args`, does that leaves this machine. Git's is
/// its push, read after git's own options; `alias` reads what a git alias
/// stands for, for a subcommand git does not have.
pub(crate) fn of_program(
    program: &str,
    args: &[String],
    alias: &dyn Fn(&str) -> Option<String>,
) -> Option<ShipKind> {
    let program = base_name(program);
    if program == "git" {
        let subcommand = git_args(args).first()?;
        return match subcommand.as_str() {
            "push" => Some(ShipKind::Push),
            sub if !GIT_COMMANDS.contains(&sub) => {
                let stands_for = alias(sub)?;
                match stands_for.strip_prefix('!') {
                    Some(shell) => (of_command(shell, &[], Looks::Every, Path::new(""), &|_| None)
                        .first())
                    .map(|ship| ship.kind),
                    None => (stands_for.split_whitespace().next() == Some("push"))
                        .then_some(ShipKind::Push),
                }
            }
            _ => None,
        };
    }
    if matches!(program, "curl" | "wget") {
        return web_write(program, args).then_some(ShipKind::Publish);
    }
    RULES
        .iter()
        .find(|rule| rule.programs.contains(&program) && matches(rule, args))
        .map(|rule| rule.kind)
}

fn matches(rule: &Rule, args: &[String]) -> bool {
    let follows = rule.words.is_empty()
        || args
            .windows(rule.words.len())
            .any(|window| window.iter().zip(rule.words).all(|(arg, word)| arg == word));
    follows
        && match rule.needs {
            Needs::Nothing => true,
            Needs::Flag(flag) => args.iter().any(|arg| arg == flag),
            Needs::ToS3 => args.last().is_some_and(|last| last.starts_with("s3://")),
        }
}

/// Which programs of a command line a check looks at.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Looks {
    Every,
    /// Only those the shim does not see, where the shim is first on PATH:
    /// programs run by a path, and the project's own commands.
    PastTheShim,
}

/// Every command of a shell command line that leaves this machine, in order,
/// as each is checked by itself. `extra` are the project's own commands that
/// do, such as `make deploy`, each matched by its leading words. `env` is what
/// the line runs under, beneath what it sets itself. A `cd` earlier on the line
/// leaves where a command lands open.
pub(crate) fn of_command(
    line: &str,
    extra: &[String],
    looks: Looks,
    cwd: &Path,
    env: &dyn Fn(&str) -> Option<String>,
) -> Vec<Ship> {
    let lines = segments(line);
    let moved = lines
        .iter()
        .any(|words| matches!(words.first().map(String::as_str), Some("cd" | "pushd")));
    lines
        .iter()
        .flat_map(|all| {
            let Some(words) = command_words(all) else {
                return Vec::new();
            };
            if let Some(script) = shell_script(words) {
                return of_command(script, extra, looks, cwd, env);
            }
            let Some((program, args)) = words.split_first() else {
                return Vec::new();
            };
            let shimmed = !program.contains('/')
                && (program == "git" || PROGRAMS.contains(&program.as_str()));
            let known = match looks == Looks::PastTheShim && shimmed {
                true => None,
                false => of_program(program, args, &|_| None),
            };
            let Some(kind) = known.or_else(|| {
                extra
                    .iter()
                    .any(|listed| leads(listed, words))
                    .then_some(ShipKind::Deploy)
            }) else {
                return Vec::new();
            };
            let set_before = &all[..all.len() - words.len()];
            let set = |name: &str| {
                let said = (set_before.iter())
                    .filter_map(|word| word.strip_prefix(name)?.strip_prefix('='))
                    .next_back();
                said.map(str::to_owned).or_else(|| env(name))
            };
            let target = (!moved || matches!(base_name(program), "curl" | "wget"))
                .then(|| target_of(program, args, &set, cwd))
                .flatten();
            vec![Ship {
                kind,
                what: words.join(" "),
                target,
            }]
        })
        .collect()
}

/// Whether `words` begin with the words of `listed`, a path such as
/// `./scripts/deploy.sh` matching one written `scripts/deploy.sh`.
pub(crate) fn leads(listed: &str, words: &[String]) -> bool {
    let plain = |word: &str| word.trim_start_matches("./").to_owned();
    let listed: Vec<String> = listed.split_whitespace().map(plain).collect();
    !listed.is_empty()
        && words.len() >= listed.len()
        && words
            .iter()
            .zip(&listed)
            .all(|(word, want)| plain(word) == *want)
}

/// When a Sikemux tool's call leaves this machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ToolShip {
    /// Every call does.
    Always(ShipKind),
    /// A call does when the database it names is marked as production and
    /// agents may change its data there.
    ProductionDatabase,
}

/// When the Sikemux tool `name` leaves this machine. A push through
/// git_commit is the core's own, checked where it pushes.
pub(crate) fn of_tool(name: &str) -> Option<ToolShip> {
    Some(match name {
        "vercel_promote" | "vercel_redeploy" | "rundeck_run" => ToolShip::Always(ShipKind::Deploy),
        "jira_create"
        | "jira_edit"
        | "jira_comment"
        | "jira_transition"
        | "jira_worklog"
        | "jira_link"
        | "bitbucket_comment"
        | "bitbucket_create_issue"
        | "bitbucket_create_pull"
        | "bitbucket_edit_pull"
        | "bitbucket_review"
        | "bitbucket_decline"
        | "bitbucket_issue_state" => ToolShip::Always(ShipKind::Publish),
        "bitbucket_merge" => ToolShip::Always(ShipKind::Merge),
        "db_query" => ToolShip::ProductionDatabase,
        _ => return None,
    })
}

/// Git's own commands, which no alias can stand in for, so only another word
/// is looked up as one.
const GIT_COMMANDS: &[&str] = &[
    "add",
    "am",
    "apply",
    "archive",
    "bisect",
    "blame",
    "branch",
    "bundle",
    "cat-file",
    "check-ignore",
    "checkout",
    "cherry",
    "cherry-pick",
    "clean",
    "clone",
    "commit",
    "commit-tree",
    "config",
    "count-objects",
    "credential",
    "describe",
    "diff",
    "diff-files",
    "diff-index",
    "diff-tree",
    "fetch",
    "for-each-ref",
    "format-patch",
    "fsck",
    "gc",
    "grep",
    "hash-object",
    "help",
    "init",
    "log",
    "ls-files",
    "ls-remote",
    "ls-tree",
    "merge",
    "merge-base",
    "mv",
    "name-rev",
    "notes",
    "pull",
    "push",
    "range-diff",
    "read-tree",
    "rebase",
    "reflog",
    "remote",
    "reset",
    "restore",
    "rev-list",
    "rev-parse",
    "revert",
    "rm",
    "shortlog",
    "show",
    "show-ref",
    "sparse-checkout",
    "stash",
    "status",
    "submodule",
    "switch",
    "symbolic-ref",
    "tag",
    "update-index",
    "update-ref",
    "var",
    "version",
    "worktree",
    "write-tree",
];

#[cfg(test)]
mod environment_tests {
    use super::*;

    fn words(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_owned).collect()
    }

    fn target(program: &str, line: &str, env: &[(&str, &str)], cwd: &Path) -> Option<Target> {
        let env = |name: &str| {
            (env.iter())
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        };
        target_of(program, &words(line), &env, cwd)
    }

    #[test]
    fn a_kube_context_is_read_only_when_nothing_can_point_it_elsewhere() {
        let cwd = Path::new("/nowhere");
        assert_eq!(
            target("kubectl", "--context dev apply -f x", &[], cwd),
            Some(Target::KubeContext { name: "dev".into() })
        );
        assert_eq!(
            target("kubectl", "--context dev --kubeconfig prod apply", &[], cwd),
            None
        );
        assert_eq!(
            target("kubectl", "--context dev --context prod apply", &[], cwd),
            None
        );
        assert_eq!(
            target("kubectl", "apply -f x", &[("HOME", "/nowhere")], cwd),
            None
        );
    }

    #[test]
    fn an_aws_profile_given_beside_credentials_is_not_trusted() {
        let cwd = Path::new("/nowhere");
        assert_eq!(
            target("aws", "ecs update-service --profile dev", &[], cwd),
            Some(Target::AwsProfile { name: "dev".into() })
        );
        assert_eq!(
            target("aws", "ecs update-service", &[("AWS_PROFILE", "dev")], cwd),
            Some(Target::AwsProfile { name: "dev".into() })
        );
        let keyed = [("AWS_PROFILE", "dev"), ("AWS_ACCESS_KEY_ID", "x")];
        assert_eq!(target("aws", "ecs update-service", &keyed, cwd), None);
        assert_eq!(target("aws", "ecs update-service", &[], cwd), None);
    }

    #[test]
    fn terraform_says_where_only_from_its_workspace() {
        let cwd = Path::new("/nowhere");
        assert_eq!(target("terraform", "apply", &[], cwd), None);
        assert_eq!(
            target("terraform", "apply", &[("TF_WORKSPACE", "dev")], cwd),
            Some(Target::TerraformWorkspace { name: "dev".into() })
        );
    }

    #[test]
    fn every_command_on_a_line_is_judged_by_itself() {
        let line = "kubectl --context dev apply -f a && kubectl --context prod apply -f b";
        let ships = of_command(line, &[], Looks::Every, Path::new("/nowhere"), &|_| None);
        let names: Vec<_> = ships.into_iter().map(|ship| ship.target).collect();
        assert_eq!(
            names,
            [
                Some(Target::KubeContext { name: "dev".into() }),
                Some(Target::KubeContext {
                    name: "prod".into()
                }),
            ]
        );
    }

    #[test]
    fn a_change_of_folder_on_the_line_leaves_where_it_lands_open() {
        let line = "cd infra && TF_WORKSPACE=dev terraform apply";
        let ships = of_command(line, &[], Looks::Every, Path::new("/nowhere"), &|_| None);
        assert_eq!(ships.len(), 1);
        assert_eq!(ships[0].target, None);
    }
}
