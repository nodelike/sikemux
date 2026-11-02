//! Actions an agent takes that leave this machine: pushing commits, merging a
//! pull request, deploying, publishing, and writing to a production database.
//! The one table of which commands and Sikemux tools they are, read by the
//! shim that stands in for those programs, by the hook that checks a shell
//! command before it runs, and by the core for its own tools.

use sikemux_wire::inbox::ShipKind;

use crate::git_guard::{base_name, command_words, git_args, segments, shell_script};

/// An action leaving this machine: what it does, and the exact command or
/// call, which the person is shown and allowing lets run once.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ship {
    pub kind: ShipKind,
    pub what: String,
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
                    Some(shell) => of_command(shell, &[]).map(|ship| ship.kind),
                    None => (stands_for.split_whitespace().next() == Some("push"))
                        .then_some(ShipKind::Push),
                }
            }
            _ => None,
        };
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

/// What a shell command line does that leaves this machine, its first such
/// command if any. `extra` are the project's own commands that do, such as
/// `make deploy`, each matched by its leading words.
pub(crate) fn of_command(line: &str, extra: &[String]) -> Option<Ship> {
    segments(line).iter().find_map(|words| {
        let words = command_words(words)?;
        if let Some(script) = shell_script(words) {
            return of_command(script, extra);
        }
        let (program, args) = words.split_first()?;
        let kind = of_program(program, args, &|_| None).or_else(|| {
            extra
                .iter()
                .any(|listed| leads(listed, words))
                .then_some(ShipKind::Deploy)
        })?;
        Some(Ship {
            kind,
            what: words.join(" "),
        })
    })
}

/// Whether `words` begin with the words of `listed`, a path such as
/// `./scripts/deploy.sh` matching one written `scripts/deploy.sh`.
fn leads(listed: &str, words: &[String]) -> bool {
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
