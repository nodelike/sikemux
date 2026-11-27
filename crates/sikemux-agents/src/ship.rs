//! Actions an agent takes that leave this machine: pushing commits, merging a
//! pull request, deploying, publishing, writing to a production database, and
//! writing to a website or a connector.
//! The one table of which commands and Sikemux tools they are, read by the
//! shim that stands in for those programs, by the hook that checks a shell
//! command before it runs, and by the core for its own tools.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sikemux_view::inbox::ShipKind;

use crate::git_guard::{
    base_name, command_words, env_folder, git_args, pieces, shell_script, Piece, Unwrapped,
};

mod alias;
mod ssh;

/// An action leaving this machine: what it does, and the exact command or
/// call, which the person is shown and allowing lets run once.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ship {
    pub kind: ShipKind,
    pub what: String,
    /// Where it lands, when hard facts say; none counts as production.
    pub target: Option<Target>,
}

/// What a write through GitHub's API changes of a repository's code, as a push or a merge would.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Lands {
    Merge {
        pull: String,
    },
    /// A branch written to; `HEAD` when the call leaves it to the repository.
    Branch {
        name: String,
    },
    /// A tag made, moved or deleted; empty when the call names none.
    Tag {
        name: String,
    },
    /// A GitHub release published, edited, deleted or having assets changed; empty when unnamed.
    Release {
        tag: String,
    },
    /// Any branch or tag.
    Any,
    /// What the call's own words don't say.
    Unread,
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
    CloudflareZone {
        zone: String,
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
    /// A database the database connector marks as production, by its connection's name, or a
    /// hosted one SQL is sent to through its provider's API, such as a Supabase project's.
    Database {
        name: String,
    },
    /// A package, image or release published for others to install, by where it goes.
    Package {
        registry: String,
    },
    /// A stack, stage or app a deploy tool names, such as a CDK stack or a Heroku app.
    Stage {
        tool: String,
        name: String,
    },
    /// A GitHub Actions workflow started by hand, by its file, with the deployment
    /// environments its jobs name, as GitHub will run it, and the inputs it was given.
    Workflow {
        file: String,
        environments: Vec<String>,
        inputs: Vec<String>,
        /// The past run or job it starts again, as `run 123`, when it isn't a new run.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rerun: Option<String>,
    },
    /// A past workflow run started again whose workflow couldn't be read, by the `owner/name`
    /// of its repository, `{owner}/{repo}` included, and the run or job it names.
    Rerun {
        repo: String,
        of: String,
    },
    /// A write through GitHub's API, by the `owner/name` of the repository it changes, as the
    /// command spells it, `{owner}/{repo}` included; empty when it names none, as a GraphQL call.
    GitHub {
        repo: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lands: Option<Lands>,
    },
    /// A tag pushed to the project's remote, which can start a release; empty when which tags
    /// can't be read.
    Tag {
        name: String,
    },
    /// Where the tool lands by its own config, since the command doesn't name it, such as the
    /// kube config's current context: `from` says how it was read, as the person reads it, and
    /// `at` where: the folder, the config file or the account, so the same name read elsewhere
    /// is another target.
    Resolved {
        from: String,
        at: String,
        target: Box<Target>,
    },
}

impl Target {
    /// The target itself, however it was found.
    pub fn bare(&self) -> &Target {
        match self {
            Self::Resolved { target, .. } => target.bare(),
            target => target,
        }
    }

    /// Where the tool's own config said it lands, when the command didn't.
    pub fn resolved_from(&self) -> Option<&str> {
        match self {
            Self::Resolved { from, .. } => Some(from),
            _ => None,
        }
    }

    /// Where a target the tool's own config named was read.
    pub fn read_at(&self) -> Option<&str> {
        match self {
            Self::Resolved { at, .. } => Some(at),
            _ => None,
        }
    }
}

fn resolved(from: impl Into<String>, at: &Path, target: Target) -> Target {
    Target::Resolved {
        from: from.into(),
        at: at.to_string_lossy().into_owned(),
        target: Box::new(target),
    }
}

impl Ship {
    /// The site it writes to, as the person reads it.
    pub fn writes_to(&self) -> Option<&str> {
        match &self.target {
            Some(Target::Site { origin }) => Some(origin),
            _ => None,
        }
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
fn current_kube_context(
    given: Option<&str>,
    env: &dyn Fn(&str) -> Option<String>,
    cwd: &Path,
) -> Option<(String, std::path::PathBuf)> {
    let files: Vec<std::path::PathBuf> = match (given, env("KUBECONFIG")) {
        (Some(file), _) => vec![cwd.join(file)],
        (None, Some(listed)) => listed
            .split(':')
            .filter(|file| !file.is_empty())
            .map(|file| cwd.join(file))
            .collect(),
        (None, None) => vec![Path::new(&env("HOME")?).join(".kube/config")],
    };
    files.iter().find_map(|file| {
        let config = std::fs::read_to_string(file).ok()?;
        config.lines().find_map(|line| {
            let name = line
                .strip_prefix("current-context:")?
                .trim()
                .trim_matches(['"', '\'']);
            (!name.is_empty()).then(|| (name.to_owned(), file.clone()))
        })
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
    let program = program_name(program);
    if let Some((program, args)) = wrapped(program, args) {
        return target_of(program, args, env, cwd);
    }
    let named = |flags: &[&str]| flag_value(args, flags);
    match program {
        "vercel" => {
            let target = named(&["--target"]);
            let preview = target
                .as_deref()
                .is_some_and(|target| !target.eq_ignore_ascii_case("production"));
            Some(Target::Vercel {
                production: !preview,
            })
        }
        "aws" => {
            if let Some(name) = flag_value(args, &["--profile"]) {
                return Some(Target::AwsProfile { name });
            }
            if let Some(key) = env("AWS_ACCESS_KEY_ID") {
                let end: String = key
                    .chars()
                    .rev()
                    .take(4)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                let keys = Target::AwsProfile {
                    name: format!("access key ending {end}"),
                };
                let at = Path::new("the environment");
                return Some(resolved("the access keys set in the environment", at, keys));
            }
            let profile = match (env("AWS_PROFILE"), env("AWS_DEFAULT_PROFILE")) {
                (Some(one), Some(other)) if one != other => return None,
                (one, other) => one.or(other),
            };
            match profile {
                Some(name) => Some(Target::AwsProfile { name }),
                None => {
                    let config = env("AWS_CONFIG_FILE")
                        .map(std::path::PathBuf::from)
                        .or_else(|| Some(Path::new(&env("HOME")?).join(".aws/config")))
                        .unwrap_or_else(|| "~/.aws/config".into());
                    let default = Target::AwsProfile {
                        name: "default".to_owned(),
                    };
                    Some(resolved("the default AWS profile", &config, default))
                }
            }
        }
        "kubectl" | "helm" => {
            if let Some(name) = flag_value(args, &["--context", "--kube-context"]) {
                if has_flag(args, &["--kubeconfig"]) {
                    return None;
                }
                return (!has_flag(
                    args,
                    &[
                        "--server",
                        "-s",
                        "--cluster",
                        "--user",
                        "--token",
                        "--kube-apiserver",
                        "--kube-token",
                    ],
                ))
                .then_some(Target::KubeContext { name });
            }
            let elsewhere = [
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
            let file = flag_value(args, &["--kubeconfig"]);
            let (name, read) = current_kube_context(file.as_deref(), env, cwd)?;
            Some(resolved(
                "the kube config's current context",
                &read,
                Target::KubeContext { name },
            ))
        }
        "curl" | "wget" if supabase_sql(args).is_some() => {
            let project = supabase_sql(args).unwrap_or_default();
            Some(Target::Database {
                name: format!("Supabase project {project}"),
            })
        }
        "curl" | "wget" => (web_addresses(args).into_iter())
            .find(|address| !address.host_str().is_some_and(local_host))
            .map(|address| Target::Site {
                origin: address.origin().ascii_serialization(),
            }),
        "gh" if gh_args(args).starts_with(&["run".to_owned(), "rerun".to_owned()]) => {
            let args = gh_args(args);
            let repo = (flag_value(&args, &["-R", "--repo"]).or_else(|| env("GH_REPO")))
                .unwrap_or_else(|| "{owner}/{repo}".to_owned());
            let named = match flag_value(&args, &["--job", "-j"]) {
                Some(job) => Rerun::Job(job),
                None => {
                    Rerun::Run(run_named(args.get(2..).unwrap_or_default()).unwrap_or_default())
                }
            };
            Some(rerun(&repo, named, env, cwd))
        }
        "gh" if gh_args(args).starts_with(&["workflow".to_owned(), "run".to_owned()]) => {
            Some(workflow(&gh_args(args), env, cwd))
        }
        "gh" if gh_args(args).first().is_some_and(|word| word == "api") => {
            let args: Vec<String> = gh_args(args).into_iter().skip(1).collect();
            let call = github_api(&args);
            let repo = call.as_ref().and_then(ApiCall::repo).unwrap_or_default();
            if let Some(file) = call.as_ref().and_then(ApiCall::dispatches) {
                return Some(Target::Workflow {
                    file,
                    environments: Vec::new(),
                    inputs: args.to_vec(),
                    rerun: None,
                });
            }
            if let Some(named) = call.as_ref().and_then(ApiCall::reruns) {
                return Some(rerun(&repo, named, env, cwd));
            }
            let lands = call.and_then(|call| call.lands(cwd));
            Some(Target::GitHub { repo, lands })
        }
        "gh" if gh_args(args).first().is_some_and(|word| word == "release") => {
            let gh = gh_args(args);
            let repo = (flag_value(&gh, &["-R", "--repo"]).or_else(|| env("GH_REPO")))
                .unwrap_or_else(|| "{owner}/{repo}".to_owned());
            let tag = gh_release_tag(gh.get(2..).unwrap_or_default());
            Some(Target::GitHub {
                repo,
                lands: Some(Lands::Release { tag }),
            })
        }
        "gh" => Some(package(GITHUB_RELEASES)),
        "npm" | "pnpm" | "yarn" if args.iter().any(|arg| arg == "publish") => Some(package("npm")),
        "changeset" => Some(package("npm")),
        "cargo" if args.iter().any(|arg| arg == "publish") => Some(package("crates.io")),
        "docker" => {
            let images: Vec<&String> = match args.first().map(String::as_str) {
                Some("push") => args
                    .iter()
                    .skip(1)
                    .filter(|arg| !arg.starts_with('-'))
                    .take(1)
                    .collect(),
                _ => (args.windows(2))
                    .filter_map(|pair| match pair {
                        [flag, image] if matches!(flag.as_str(), "-t" | "--tag") => Some(image),
                        _ => None,
                    })
                    .collect(),
            };
            let local = |image: &&String| {
                let host = image.split('/').next().unwrap_or_default();
                let host = host.split(':').next().unwrap_or(host);
                image.contains('/') && local_host(host)
            };
            (images.is_empty() || !images.iter().all(local))
                .then(|| package("a container registry"))
        }
        "eas"
            if args
                .iter()
                .any(|arg| arg == "submit" || arg == "--auto-submit") =>
        {
            Some(package("the app stores"))
        }
        "eas" => Some(stage(
            "EAS",
            &named(&["--branch", "--channel"]).unwrap_or_default(),
        )),
        "twine" | "poetry" | "uv" => Some(package("PyPI")),
        "gem" => Some(package("RubyGems")),
        "fly" | "flyctl" => {
            if let Some(app) = named(&["--app", "-a"]) {
                return Some(stage("Fly", &app));
            }
            let config = named(&["--config", "-c"]).unwrap_or_else(|| "fly.toml".to_owned());
            let at = cwd.join(&config);
            Some(from_file("Fly", file_app("Fly", &at), &config, &at))
        }
        "wrangler" => {
            if let Some(name) = named(&["--env", "-e"]) {
                return Some(stage("Cloudflare", &name));
            }
            let name = file_app("Cloudflare", &cwd.join("wrangler.toml"))
                .or_else(|| file_app("Cloudflare", &cwd.join("wrangler.json")));
            Some(from_file("Cloudflare", name, "the wrangler config", cwd))
        }
        "firebase" => {
            if let Some(name) = named(&["--project", "-P"]) {
                return Some(stage("Firebase", &name));
            }
            let name = file_app("Firebase", &cwd.join(".firebaserc"));
            Some(from_file(
                "Firebase",
                name,
                ".firebaserc",
                &cwd.join(".firebaserc"),
            ))
        }
        "railway" => Some(stage(
            "Railway",
            &named(&["--environment", "-e"]).unwrap_or_default(),
        )),
        "netlify" => Some(stage(
            "Netlify",
            &named(&["--site"]).unwrap_or_else(|| "production".to_owned()),
        )),
        "cdk" => {
            let stacks: Vec<&String> = match args
                .iter()
                .position(|arg| arg == "deploy" || arg == "destroy")
            {
                Some(at) => args
                    .iter()
                    .skip(at + 1)
                    .filter(|arg| !arg.starts_with('-'))
                    .collect(),
                None => Vec::new(),
            };
            let name = match stacks.as_slice() {
                [stack] => (*stack).clone(),
                _ => String::new(),
            };
            Some(stage("CDK", &name))
        }
        "sst" => Some(match named(&["--stage"]) {
            Some(name) => stage("SST", &name),
            None => {
                let file = std::fs::read_to_string(cwd.join(".sst/stage")).ok();
                let name = file
                    .map(|name| name.trim().to_owned())
                    .filter(|name| !name.is_empty());
                from_file("SST", name, ".sst/stage", &cwd.join(".sst/stage"))
            }
        }),
        "sam" => Some(match named(&["--config-env"]) {
            Some(name) => stage("SAM", &name),
            None => resolved("SAM's default config", cwd, stage("SAM", "default")),
        }),
        "pulumi" => Some(stage(
            "Pulumi",
            &named(&["--stack", "-s"])
                .or_else(|| env("PULUMI_STACK"))
                .unwrap_or_default(),
        )),
        "serverless" | "sls" => Some(match named(&["--stage", "-s"]) {
            Some(name) if name.contains('$') => stage("Serverless", ""),
            Some(name) => stage("Serverless", &name),
            None => match serverless_stage(cwd) {
                name if name.is_empty() => stage("Serverless", ""),
                name => resolved("the serverless config", cwd, stage("Serverless", &name)),
            },
        }),
        "heroku" => Some(match named(&["--app", "-a"]) {
            Some(app) => stage("Heroku", &app),
            None => {
                let url = crate::git::Git::quick(cwd)
                    .trimmed(&["remote", "get-url", "heroku"])
                    .ok();
                let app = url.as_deref().and_then(|url| {
                    let app = url.split_once("git.heroku.com/")?.1;
                    Some(
                        app.trim_end_matches('/')
                            .trim_end_matches(".git")
                            .to_owned(),
                    )
                });
                from_file("Heroku", app, "the heroku git remote", cwd)
            }
        }),
        "dokku" => Some(stage(
            "Dokku",
            &named(&["--app"])
                .or_else(|| args.get(1).filter(|arg| !arg.starts_with('-')).cloned())
                .unwrap_or_default(),
        )),
        "supabase" | "prisma" => migrates_at(program, args, env, cwd).map(|name| {
            stage(
                match program {
                    "supabase" => "Supabase",
                    _ => "Prisma",
                },
                &name,
            )
        }),
        "psql" | "mysql" => database_at(args, env).map(|host| Target::Site {
            origin: format!("{program}://{host}"),
        }),
        "terraform" => {
            let dir =
                flag_value(args, &["-chdir"]).map_or_else(|| cwd.to_owned(), |dir| cwd.join(dir));
            if let Some(name) = env("TF_WORKSPACE") {
                return Some(Target::TerraformWorkspace { name });
            }
            let data =
                env("TF_DATA_DIR").map_or_else(|| dir.join(".terraform"), |data| dir.join(data));
            let selected = std::fs::read_to_string(data.join("environment"))
                .ok()
                .map(|name| name.trim().to_owned())
                .filter(|name| !name.is_empty());
            Some(match selected {
                Some(name) => resolved(
                    "the workspace selected in this folder",
                    &data,
                    Target::TerraformWorkspace { name },
                ),
                None => resolved(
                    "Terraform's default workspace",
                    &data,
                    Target::TerraformWorkspace {
                        name: "default".to_owned(),
                    },
                ),
            })
        }
        program if ssh::PROGRAMS.contains(&program) => ssh::changed_host(program, args)
            .filter(|host| !host.contains(['$', '`']))
            .map(|host| stage(SSH, &host)),
        _ => None,
    }
}

/// The name of the program a command's first word runs, one spelled short in full.
fn program_name(word: &str) -> &str {
    match base_name(word) {
        "vc" => "vercel",
        name => name,
    }
}

/// The program and arguments a runner such as `npx` or `pnpm dlx` starts.
pub fn wrapped<'a>(program: &str, args: &'a [String]) -> Option<(&'a str, &'a [String])> {
    let skip = match program {
        "npx" | "pnpx" | "bunx" => 0,
        "pnpm" | "yarn" | "bun" | "npm" => {
            let first = args.first()?;
            match first.as_str() {
                "dlx" | "exec" | "x" => 1,
                _ => return None,
            }
        }
        _ => return None,
    };
    let rest = args.get(skip..)?;
    let at = rest.iter().position(|arg| !arg.starts_with('-'))?;
    let (program, args) = rest.get(at..)?.split_first()?;
    let program = program.rsplit('/').next().unwrap_or(program);
    let program = program
        .split('@')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(program);
    Some((program_name(program), args))
}

/// The stage a tool's own file names, as read from `file`, or an unnamed one.
fn from_file(tool: &str, name: Option<String>, file: &str, at: &Path) -> Target {
    match name {
        Some(name) => resolved(file, at, stage(tool, &name)),
        None => stage(tool, ""),
    }
}

fn stage(tool: &str, name: &str) -> Target {
    Target::Stage {
        tool: tool.to_owned(),
        name: name.to_owned(),
    }
}

fn package(registry: &str) -> Target {
    Target::Package {
        registry: registry.to_owned(),
    }
}

/// The app a deploy tool's config file names, by the tool it belongs to.
fn file_app(tool: &str, file: &Path) -> Option<String> {
    app_named(tool, &std::fs::read_to_string(file).ok()?)
}

/// The app a deploy tool's config names: Fly's `app`, Cloudflare's worker `name`, Firebase's
/// default project.
pub fn app_named(tool: &str, text: &str) -> Option<String> {
    if tool == "Firebase" {
        let value: serde_json::Value = serde_json::from_str(text).ok()?;
        return value
            .get("projects")?
            .get("default")?
            .as_str()
            .map(str::to_owned);
    }
    let key = match tool {
        "Fly" => "app",
        _ => "name",
    };
    text.lines().find_map(|line| {
        let line = line.trim().trim_end_matches(',');
        let rest = (line.strip_prefix(&format!("\"{key}\"")))
            .or_else(|| line.strip_prefix(key))?
            .trim_start();
        let value = rest
            .strip_prefix([':', '='])?
            .trim()
            .trim_matches(['"', '\'']);
        (!value.is_empty() && !value.contains(['{', '['])).then(|| value.to_owned())
    })
}

/// The stage serverless.yml in `cwd` deploys to when no flag names one.
/// Empty when the config can't say: one in code, a stage set from a variable, or none found.
/// Serverless's own default is dev.
fn serverless_stage(cwd: &Path) -> String {
    let config = (std::fs::read_to_string(cwd.join("serverless.yml")))
        .or_else(|_| std::fs::read_to_string(cwd.join("serverless.yaml")));
    let Ok(config) = config else {
        return String::new();
    };
    let said = config.lines().find_map(|line| {
        let value = line.trim().strip_prefix("stage:")?.trim();
        Some(value.trim_matches(['"', '\'']).to_owned())
    });
    match said {
        None => "dev".to_owned(),
        Some(value) if value.is_empty() || value.contains('$') => String::new(),
        Some(value) => value,
    }
}

/// The database a migration tool changes, by its host or linked project; empty when it can't
/// be read, and none when it is this computer. Addresses are read, never printed.
fn migrates_at(
    program: &str,
    args: &[String],
    env: &dyn Fn(&str) -> Option<String>,
    cwd: &Path,
) -> Option<String> {
    if program == "supabase" {
        if has_flag(args, &["--local"]) {
            return None;
        }
        if has_flag(args, &["--db-url"]) {
            return database_at(args, env).map(|host| first_label(&host));
        }
        let linked = std::fs::read_to_string(cwd.join("supabase/.temp/project-ref")).ok();
        return Some(
            linked
                .map(|name| name.trim().to_owned())
                .unwrap_or_default(),
        );
    }
    if has_flag(args, &["--url"]) || env("DATABASE_URL").is_some() {
        let address = flag_value(args, &["--url"]).or_else(|| env("DATABASE_URL"));
        return host_of_address(address.as_deref()?).map_or(Some(String::new()), |host| {
            (!local_host(&host)).then(|| first_label(&host))
        });
    }
    let file = ["prisma/.env", ".env"]
        .iter()
        .find_map(|file| std::fs::read_to_string(cwd.join(file)).ok());
    let address = file.as_deref().and_then(|text| {
        text.lines().find_map(|line| {
            let value = line.trim().strip_prefix("DATABASE_URL=")?;
            Some(value.trim().trim_matches(['"', '\'']).to_owned())
        })
    });
    match address.as_deref().and_then(host_of_address) {
        Some(host) if local_host(&host) => None,
        Some(host) => Some(first_label(&host)),
        None => Some(String::new()),
    }
}

/// A database host's own name, before its domain, which says what it is: `staging-db` of
/// `staging-db.acme.dev`.
fn first_label(host: &str) -> String {
    host.split('.').next().unwrap_or(host).to_owned()
}

fn host_of_address(address: &str) -> Option<String> {
    let host = url::Url::parse(address).ok()?.host_str()?.to_owned();
    (!host.is_empty()).then_some(host)
}

/// The database server a client or migration tool reaches, from its flags, a connection
/// address among its arguments, or the environment; none when that is this computer.
fn database_at(args: &[String], env: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    let address = (args.iter())
        .find(|arg| {
            arg.contains("://") && (arg.starts_with("postgres") || arg.starts_with("mysql"))
        })
        .cloned()
        .or_else(|| flag_value(args, &["--db-url"]));
    let host = match address {
        Some(address) => url::Url::parse(&address).ok()?.host_str()?.to_owned(),
        None => match flag_value(args, &["-h", "--host"]).or_else(|| env("PGHOST")) {
            Some(host) => host,
            None => {
                let address = env("DATABASE_URL")?;
                url::Url::parse(&address).ok()?.host_str()?.to_owned()
            }
        },
    };
    (!local_host(&host) && !host.starts_with('/')).then_some(host)
}

/// What a workflow's file says it does: the deployment environments its jobs name, whether
/// it publishes a release or a package, and whether it deploys by a step known to.
pub struct WorkflowRead {
    pub environments: Vec<String>,
    pub publishes: bool,
    pub deploys: bool,
}

/// Steps and commands in a workflow that put something live.
const DEPLOY_STEPS: &[&str] = &[
    "actions/deploy-pages",
    "actions-gh-pages",
    "github-pages-deploy-action",
    "vercel-action",
    "vercel deploy",
    "vercel --prod",
    "netlify deploy",
    "actions-netlify",
    "flyctl-actions",
    "flyctl deploy",
    "fly deploy",
    "wrangler-action",
    "wrangler deploy",
    "wrangler pages deploy",
    "amazon-ecs-deploy",
    "aws-actions/",
    "deploy-cloudrun",
    "deploy-appengine",
    "webapps-deploy",
    "action-hosting-deploy",
    "firebase deploy",
    "kubectl apply",
    "helm upgrade",
    "terraform apply",
    "pulumi up",
    "cdk deploy",
    "serverless deploy",
    "railway up",
    "heroku",
    "eas update",
    "eas submit",
    "docker push",
    "s3 sync",
    "rsync ",
    "scp ",
];

pub fn read_workflow(text: &str) -> WorkflowRead {
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    let environments = (lines.iter().enumerate())
        .filter_map(|(at, line)| {
            let value = line.strip_prefix("environment:")?.trim();
            let value = match value.is_empty() {
                true => lines.get(at + 1)?.strip_prefix("name:")?.trim(),
                false => value,
            };
            let value = value.trim_matches(['"', '\'']);
            (!value.is_empty() && !value.starts_with('{')).then(|| value.to_owned())
        })
        .collect();
    let publishes = [
        "gh release",
        "action-gh-release",
        "npm publish",
        "pnpm publish",
        "yarn publish",
        "cargo publish",
        "twine upload",
        "changeset publish",
        "release:",
        "releases:",
    ]
    .iter()
    .any(|says| text.contains(says));
    let deploys = DEPLOY_STEPS.iter().any(|says| text.contains(says))
        || run_lines(text).iter().any(|line| line_ships(line));
    WorkflowRead {
        environments,
        publishes,
        deploys,
    }
}

/// The commands a workflow's steps run, each line of a `run:` block by itself.
fn run_lines(text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut block: Option<usize> = None;
    for line in text.lines() {
        let indent = line.len() - line.trim_start().len();
        if let Some(at) = block {
            if line.trim().is_empty() || indent > at {
                lines.push(line.trim().to_owned());
                continue;
            }
            block = None;
        }
        let trimmed = line.trim_start().trim_start_matches("- ");
        if let Some(run) = trimmed.strip_prefix("run:") {
            let run = run.trim();
            match run.starts_with('|') || run.starts_with('>') {
                true => block = Some(indent),
                false => lines.push(run.to_owned()),
            }
        }
    }
    lines.retain(|line| !line.is_empty() && !line.starts_with('#'));
    lines
}

/// Whether a workflow's command sends something out as the gate would read it typed, or names
/// production anywhere, a program behind a variable included.
fn line_ships(line: &str) -> bool {
    let ships = of_command(line, &[], Looks::Every, Path::new("/"), &|_| None);
    let sends = ships
        .iter()
        .any(|ship| !matches!(ship.kind, ShipKind::Push | ShipKind::Merge));
    sends || says_production(line)
}

/// The workflow files a workflow's jobs call in the same repository, by path.
pub fn called_workflows(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let used = line
                .trim()
                .trim_start_matches("- ")
                .strip_prefix("uses:")?
                .trim();
            let path = used.trim_matches(['"', '\'']).strip_prefix("./")?;
            path.starts_with(".github/workflows/")
                .then(|| path.to_owned())
        })
        .collect()
}

/// What a rerun starts again: a past run, or one job of it, by its id.
enum Rerun {
    Run(String),
    Job(String),
}

/// The run id among `gh run rerun`'s words past the subcommand.
fn run_named(words: &[String]) -> Option<String> {
    let mut words = words.iter();
    while let Some(word) = words.next() {
        match word.as_str() {
            "-R" | "--repo" | "-j" | "--job" => {
                words.next();
            }
            word if word.starts_with('-') => {}
            word => return Some(word.to_owned()),
        }
    }
    None
}

/// The workflow a past run of `repo` is of, as GitHub runs it again: its file at the commit
/// the run was of, asked of GitHub as the person's `gh`. A run whose file, or whose
/// environments, can't be read that way stays a rerun by its id.
fn rerun(repo: &str, named: Rerun, env: &dyn Fn(&str) -> Option<String>, cwd: &Path) -> Target {
    let gh = |endpoint: &str, jq: Option<&str>| -> Option<String> {
        let mut command = sikemux_process::user_environment::command("gh");
        command
            .current_dir(cwd)
            .env("GH_PROMPT_DISABLED", "1")
            .args(["api", endpoint]);
        match jq {
            Some(jq) => command.args(["--jq", jq]),
            None => command.args(["-H", "Accept: application/vnd.github.raw"]),
        };
        for name in ["GH_HOST", "GH_REPO"] {
            match env(name) {
                Some(value) => command.env(name, value),
                None => command.env_remove(name),
            };
        }
        let output = (command.output().ok()).filter(|output| output.status.success())?;
        String::from_utf8(output.stdout).ok()
    };
    let of = match &named {
        Rerun::Run(run) => format!("run {run}"),
        Rerun::Job(job) => format!("job {job}"),
    };
    let read = || {
        let run = match named {
            Rerun::Run(run) => run,
            Rerun::Job(job) => gh(&format!("repos/{repo}/actions/jobs/{job}"), Some(".run_id"))?
                .trim()
                .to_owned(),
        };
        let found = gh(
            &format!("repos/{repo}/actions/runs/{run}"),
            Some(r#".path + " " + .head_sha"#),
        )?;
        let (path, sha) = found.trim().split_once(' ')?;
        let path = path.split('@').next()?;
        let file = path.strip_prefix(".github/workflows/")?;
        let at = |path: &str| gh(&format!("repos/{repo}/contents/{path}?ref={sha}"), None);
        let mut text = at(path)?;
        for called in called_workflows(&text.clone()) {
            text.push('\n');
            text.push_str(&at(&called)?);
        }
        let environments = read_workflow(&text).environments;
        if environments.iter().any(|name| name.contains("${{")) {
            return None;
        }
        Some(Target::Workflow {
            file: file.to_owned(),
            environments,
            inputs: Vec::new(),
            rerun: Some(of.clone()),
        })
    };
    read().unwrap_or_else(|| Target::Rerun {
        repo: repo.to_owned(),
        of,
    })
}

/// The workflow `gh workflow run` starts, read as GitHub will run it: from the ref `--ref`
/// names, or the default branch, as the remote last showed it. Read is false when it names
/// another repository or its file can't be read that way.
fn workflow(args: &[String], env: &dyn Fn(&str) -> Option<String>, cwd: &Path) -> Target {
    let at = (args.windows(2).position(|pair| pair == ["workflow", "run"])).unwrap_or_default();
    let rest = args.get(at + 2..).unwrap_or_default();
    let mut named = None;
    let mut inputs = Vec::new();
    let mut words = rest.iter();
    while let Some(word) = words.next() {
        match word.as_str() {
            "-f" | "-F" | "--field" | "--raw-field" | "--ref" | "-r" | "-R" | "--repo"
            | "--json" => {
                if let Some(value) = words.next() {
                    if !matches!(word.as_str(), "--ref" | "-r" | "-R" | "--repo") {
                        inputs.push(value.clone());
                    }
                }
            }
            word if word.starts_with('-') => inputs.push(word.to_owned()),
            word if named.is_none() => named = Some(word.to_owned()),
            word => inputs.push(word.to_owned()),
        }
    }
    let file = named.clone().unwrap_or_default();
    let unread = Target::Workflow {
        file,
        environments: Vec::new(),
        inputs: inputs.clone(),
        rerun: None,
    };
    let elsewhere = has_flag(args, &["-R", "--repo"]) || env("GH_REPO").is_some();
    let Some(named) = named.filter(|_| !elsewhere) else {
        return unread;
    };
    let git = || crate::git::Git::quick(cwd);
    let reference = match flag_value(args, &["--ref", "-r"]) {
        Some(reference) => format!("origin/{reference}"),
        None => match git().trimmed(&["rev-parse", "--abbrev-ref", "origin/HEAD"]) {
            Ok(reference) => reference,
            Err(_) => return unread,
        },
    };
    let Ok(listed) = git().trimmed(&["ls-tree", "--name-only", &reference, ".github/workflows/"])
    else {
        return unread;
    };
    let found = listed.lines().find_map(|path| {
        let name = path.rsplit('/').next()?;
        let text = git()
            .trimmed(&["show", &format!("{reference}:{path}")])
            .ok()?;
        let titled = text.lines().any(|line| {
            line.strip_prefix("name:")
                .is_some_and(|title| title.trim().trim_matches(['"', '\'']) == named)
        });
        (name == named || titled).then(|| (name.to_owned(), text))
    });
    let Some((file, mut text)) = found else {
        return unread;
    };
    for called in called_workflows(&text.clone()) {
        if let Ok(more) = git().trimmed(&["show", &format!("{reference}:{called}")]) {
            text.push('\n');
            text.push_str(&more);
        }
    }
    let read = read_workflow(&text);
    Target::Workflow {
        file,
        environments: read.environments,
        inputs,
        rerun: None,
    }
}

/// The programs besides git the shim stands in for, so a deploy one of them makes, or a write
/// to a database, reaching a production place is checked before it runs, whatever script
/// starts it.
pub(crate) const PROGRAMS: &[&str] = &[
    "gh",
    "vercel",
    "vc",
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
    "cdk",
    "heroku",
    "sst",
    "sam",
    "dokku",
    "psql",
    "mysql",
];

/// Programs that send something out only when what they reach is another computer: a
/// database server, or a registry that isn't this computer's own.
const ONLY_ELSEWHERE: &[&str] = &["psql", "mysql", "supabase", "prisma", "docker"];

/// Whether `program`'s action leaves this machine, once where it lands is known.
pub(crate) fn leaves_machine(program: &str, args: &[String], target: Option<&Target>) -> bool {
    let program = base_name(program);
    let program = wrapped(program, args).map_or(program, |(program, _)| program);
    target.is_some() || !ONLY_ELSEWHERE.contains(&program)
}

/// What a rule's words need besides themselves to take the action.
enum Needs {
    Nothing,
    /// The command also passes this flag.
    Flag(&'static str),
    /// The command's last word names an S3 bucket, so it writes there.
    ToS3,
    /// `gh api` with a method or fields that write.
    ApiWrite,
    /// A command other than one that only reads.
    NotReading(&'static [&'static str]),
    /// A database client given something other than statements that only read.
    NotQuerying,
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
    rule(&["gh"], &["run", "rerun"], Needs::Nothing, ShipKind::Deploy),
    rule(&["vercel"], &[], Needs::Flag("--prod"), ShipKind::Deploy),
    rule(&["vercel"], &["promote"], Needs::Nothing, ShipKind::Deploy),
    rule(&["vercel"], &["redeploy"], Needs::Nothing, ShipKind::Deploy),
    rule(
        &["vercel"],
        &["alias", "set"],
        Needs::Nothing,
        ShipKind::Deploy,
    ),
    rule(
        &["vercel"],
        &["alias", "rm"],
        Needs::Nothing,
        ShipKind::Deploy,
    ),
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
    rule(&["twine"], &["upload"], Needs::Nothing, ShipKind::Publish),
    rule(&["poetry"], &["publish"], Needs::Nothing, ShipKind::Publish),
    rule(&["gem"], &["push"], Needs::Nothing, ShipKind::Publish),
    rule(&["cdk"], &["deploy"], Needs::Nothing, ShipKind::Deploy),
    rule(&["cdk"], &["destroy"], Needs::Nothing, ShipKind::Deploy),
    rule(
        &["heroku"],
        &["container:release"],
        Needs::Nothing,
        ShipKind::Deploy,
    ),
    rule(
        &["heroku"],
        &["releases:rollback"],
        Needs::Nothing,
        ShipKind::Deploy,
    ),
    rule(&["vercel"], &["rollback"], Needs::Nothing, ShipKind::Deploy),
    rule(&["vercel"], &[], Needs::Flag("--target"), ShipKind::Deploy),
    rule(
        &["netlify"],
        &["deploy"],
        Needs::Flag("-p"),
        ShipKind::Deploy,
    ),
    rule(KUBECTL, &["create"], Needs::Nothing, ShipKind::Deploy),
    rule(KUBECTL, &["replace"], Needs::Nothing, ShipKind::Deploy),
    rule(KUBECTL, &["edit"], Needs::Nothing, ShipKind::Deploy),
    rule(HELM, &["delete"], Needs::Nothing, ShipKind::Deploy),
    rule(
        AWS,
        &["cloudformation", "update-stack"],
        Needs::Nothing,
        ShipKind::Deploy,
    ),
    rule(
        AWS,
        &["cloudformation", "create-stack"],
        Needs::Nothing,
        ShipKind::Deploy,
    ),
    rule(
        AWS,
        &["cloudformation", "delete-stack"],
        Needs::Nothing,
        ShipKind::Deploy,
    ),
    rule(AWS, &["s3", "rm"], Needs::Nothing, ShipKind::Deploy),
    rule(AWS, &["s3", "rb"], Needs::Nothing, ShipKind::Deploy),
    rule(
        AWS,
        &["lambda", "update-function-configuration"],
        Needs::Nothing,
        ShipKind::Deploy,
    ),
    rule(&["gh"], &["api"], Needs::ApiWrite, ShipKind::Publish),
    rule(
        &["eas"],
        &["build"],
        Needs::Flag("--auto-submit"),
        ShipKind::Publish,
    ),
    rule(&["sst"], &["deploy"], Needs::Nothing, ShipKind::Deploy),
    rule(&["sst"], &["remove"], Needs::Nothing, ShipKind::Deploy),
    rule(&["sam"], &["deploy"], Needs::Nothing, ShipKind::Deploy),
    rule(&["sam"], &["delete"], Needs::Nothing, ShipKind::Deploy),
    rule(&["uv"], &["publish"], Needs::Nothing, ShipKind::Publish),
    rule(
        &["changeset"],
        &["publish"],
        Needs::Nothing,
        ShipKind::Publish,
    ),
    rule(
        &["supabase"],
        &["db", "push"],
        Needs::Nothing,
        ShipKind::ProductionWrite,
    ),
    rule(
        &["prisma"],
        &["migrate", "deploy"],
        Needs::Nothing,
        ShipKind::ProductionWrite,
    ),
    rule(
        &["prisma"],
        &["db", "push"],
        Needs::Nothing,
        ShipKind::ProductionWrite,
    ),
    rule(
        &["psql", "mysql"],
        &[],
        Needs::NotQuerying,
        ShipKind::Publish,
    ),
    rule(
        &["dokku"],
        &[],
        Needs::NotReading(&[
            "apps:list",
            "apps:exists",
            "logs",
            "config",
            "config:get",
            "config:show",
            "domains",
            "help",
            "version",
            "url",
            "urls",
        ]),
        ShipKind::Deploy,
    ),
];

/// What `program`, run with `args`, does that leaves this machine. Git's is
/// its push, read after git's own options; `alias` reads what a git alias
/// stands for, for a subcommand git does not have.
pub(crate) fn of_program(
    program: &str,
    args: &[String],
    cwd: &Path,
    alias: &dyn Fn(&str) -> Option<String>,
) -> Option<ShipKind> {
    let program = program_name(program);
    if program == "git" {
        if git_global_help(args) {
            return None;
        }
        let sub = git_args(args);
        let (subcommand, sub_args) = sub.split_first()?;
        return match subcommand.as_str() {
            "push"
                if has_program_flag(
                    sub_args,
                    &["-n", "--dry-run", "--help"],
                    GIT_PUSH_SWITCHES,
                    false,
                ) =>
            {
                None
            }
            "send-pack"
                if has_program_flag(
                    sub_args,
                    &["--dry-run", "--help"],
                    GIT_PUSH_SWITCHES,
                    false,
                ) =>
            {
                None
            }
            "push" | "send-pack" => Some(ShipKind::Push),
            "subtree"
                if !has_program_flag(sub_args, &["--help"], GIT_PUSH_SWITCHES, false)
                    && sub_args.iter().any(|arg| arg == "push") =>
            {
                Some(ShipKind::Push)
            }
            sub if !GIT_COMMANDS.contains(&sub) => {
                if has_program_flag(sub_args, &["--help"], GIT_PUSH_SWITCHES, false) {
                    return None;
                }
                let stands_for = alias(sub)?;
                match stands_for.strip_prefix('!') {
                    Some(shell) => (of_command(shell, &[], Looks::Every, cwd, &|_| None).first())
                        .map(|ship| ship.kind),
                    None => (stands_for.split_whitespace().next() == Some("push"))
                        .then_some(ShipKind::Push),
                }
            }
            _ => None,
        };
    }
    if matches!(program, "curl" | "wget") {
        if supabase_sql(args).is_some() {
            return (!sends_only_reads(program, args)).then_some(ShipKind::ProductionWrite);
        }
        return web_write(program, args).then_some(ShipKind::Publish);
    }
    if ssh::PROGRAMS.contains(&program) {
        return ssh::changed_host(program, args).map(|_| ShipKind::Deploy);
    }
    if let Some((program, args)) = wrapped(program, args) {
        return of_program(program, args, cwd, alias);
    }
    if program_skips_action(program, args) {
        return None;
    }
    let first = args
        .iter()
        .find(|arg| !arg.starts_with('-'))
        .map(String::as_str);
    if program == "vercel" && vercel_takes_away(args) {
        return Some(ShipKind::Deploy);
    }
    if program == "vercel" && first.is_some_and(|first| VERCEL_READS.contains(&first)) {
        return None;
    }
    if matches!(program, "kubectl" | "helm") && !changes_cluster(program, args) {
        return None;
    }
    if matches!(program, "serverless" | "sls")
        && args.windows(2).any(|pair| pair == ["deploy", "list"])
    {
        return None;
    }
    if program == "vercel" && matches!(first, Some("rm" | "remove")) {
        return Some(ShipKind::Deploy);
    }
    if program == "gh" {
        let gh = gh_args(args);
        if let [first, sub, ..] = gh.as_slice() {
            if first == "release"
                && !sub.starts_with('-')
                && !GH_RELEASE_READS.contains(&sub.as_str())
            {
                return Some(ShipKind::Publish);
            }
        }
    }
    RULES
        .iter()
        .find(|rule| rule.programs.contains(&program) && matches(rule, args, cwd))
        .map(|rule| rule.kind)
}

/// Where `aws s3 cp`, `sync` or `mv` writes: the second of its paths, past the flags.
fn s3_destination(args: &[String]) -> Option<&String> {
    let switches = [
        "--recursive",
        "--delete",
        "--dryrun",
        "--quiet",
        "--only-show-errors",
        "--no-progress",
        "--exact-timestamps",
        "--size-only",
        "--follow-symlinks",
        "--no-follow-symlinks",
        "--no-guess-mime-type",
        "--force-glacier-transfer",
        "--ignore-glacier-warnings",
        "--debug",
        "--no-verify-ssl",
        "--no-paginate",
        "--no-sign-request",
    ];
    let at = args
        .iter()
        .position(|arg| matches!(arg.as_str(), "cp" | "sync" | "mv"))?;
    let mut paths = Vec::new();
    let mut words = args.iter().skip(at + 1);
    while let Some(word) = words.next() {
        if word.starts_with("--") && !word.contains('=') && !switches.contains(&word.as_str()) {
            words.next();
        } else if !word.starts_with('-') {
            paths.push(word);
        }
    }
    paths.get(1).copied()
}

/// A `gh api` call: its method, as gh sends it, and the endpoint, without a leading slash.
pub struct ApiCall {
    pub method: String,
    pub endpoint: String,
    fields: Vec<String>,
    /// The file its body is read from.
    input: Option<String>,
}

impl ApiCall {
    /// The `owner/name` of the repository the endpoint changes, when it names one.
    pub fn repo(&self) -> Option<String> {
        let mut parts = self.path().split('/');
        let ("repos", Some(owner), Some(name)) = (parts.next()?, parts.next(), parts.next()) else {
            return None;
        };
        Some(format!("{owner}/{name}"))
    }

    fn path(&self) -> &str {
        self.endpoint.split(['?', '#']).next().unwrap_or_default()
    }

    /// The parts of the endpoint past the repository's own, given by name or by number.
    fn under_repo(&self) -> Vec<&str> {
        let parts: Vec<&str> = self.path().trim_end_matches('/').split('/').collect();
        match parts.as_slice() {
            ["repos", _, _, rest @ ..] | ["repositories", _, rest @ ..] => rest.to_vec(),
            _ => Vec::new(),
        }
    }

    /// The past run, or job of one, it starts again.
    fn reruns(&self) -> Option<Rerun> {
        match self.under_repo().as_slice() {
            ["actions", "runs", run, "rerun" | "rerun-failed-jobs"] => {
                Some(Rerun::Run((*run).to_owned()))
            }
            ["actions", "jobs", job, "rerun"] => Some(Rerun::Job((*job).to_owned())),
            _ => None,
        }
    }

    /// The workflow it starts.
    fn dispatches(&self) -> Option<String> {
        match self.under_repo().as_slice() {
            ["actions", "workflows", file, "dispatches"] => Some((*file).to_owned()),
            _ => None,
        }
    }

    fn lands(&self, cwd: &Path) -> Option<Lands> {
        let of_ref = |name: &str| {
            let name = name.strip_prefix("refs/").unwrap_or(name);
            if let Some(tag) = name.strip_prefix("tags/") {
                return Some(Lands::Tag {
                    name: tag.to_owned(),
                });
            }
            (name.strip_prefix("heads/")).map(|branch| Lands::Branch {
                name: branch.to_owned(),
            })
        };
        let branch = |field: &str| match self.field(field, cwd) {
            Ok(name) => {
                let name = name.unwrap_or_else(|| "HEAD".to_owned());
                Lands::Branch {
                    name: name.strip_prefix("refs/heads/").unwrap_or(&name).to_owned(),
                }
            }
            Err(Unread) => Lands::Unread,
        };
        if self.endpoint == "graphql" {
            return self.graphql_lands(cwd);
        }
        match self.under_repo().as_slice() {
            ["pulls", pull, "merge"] => Some(Lands::Merge {
                pull: (*pull).to_owned(),
            }),
            ["git", "refs"] => match self.field("ref", cwd) {
                Ok(Some(name)) => of_ref(&name),
                _ => Some(Lands::Unread),
            },
            ["git", "refs", name @ ..] => of_ref(&name.join("/")),
            ["contents", ..] | ["merge-upstream"] => Some(branch("branch")),
            ["merges"] => Some(branch("base")),
            ["branches", from, "rename"] => Some(Lands::Branch {
                name: (*from).to_owned(),
            }),
            ["releases", ..] => self.release_lands(cwd),
            ["actions", "runs", _, "pending_deployments"] => Some(Lands::Any),
            _ => None,
        }
    }

    /// A release published, edited, deleted or having its assets changed.
    fn release_lands(&self, cwd: &Path) -> Option<Lands> {
        let making = match self.under_repo().as_slice() {
            ["releases"] => true,
            ["releases", "generate-notes"] => return None,
            ["releases", _] => false,
            ["releases", ..] => {
                return Some(Lands::Release { tag: String::new() });
            }
            _ => return None,
        };
        if self.method == "DELETE" {
            return Some(Lands::Release { tag: String::new() });
        }
        let field = |name: &str| self.field(name, cwd);
        let (Ok(tag), Ok(draft)) = (field("tag_name"), field("draft")) else {
            return Some(Lands::Unread);
        };
        if making && draft.as_deref() == Some("true") {
            return None;
        }
        Some(Lands::Release {
            tag: tag.unwrap_or_default(),
        })
    }

    fn graphql_lands(&self, cwd: &Path) -> Option<Lands> {
        const MUTATIONS: [&str; 10] = [
            "mergePullRequest",
            "enablePullRequestAutoMerge",
            "enqueuePullRequest",
            "createCommitOnBranch",
            "createRef",
            "updateRef",
            "updateRefs",
            "deleteRef",
            "mergeBranch",
            "revertPullRequest",
        ];
        let Ok(query) = self.said("query", cwd, &|file| {
            std::fs::read_to_string(cwd.join(file)).ok()
        }) else {
            return Some(Lands::Unread);
        };
        let query = query.unwrap_or_default();
        (query.split(|c: char| !c.is_ascii_alphanumeric() && c != '_'))
            .any(|word| MUTATIONS.contains(&word))
            .then_some(Lands::Any)
    }

    /// What it sends as the field `name`, none when it doesn't send it.
    fn field(&self, name: &str, cwd: &Path) -> Result<Option<String>, Unread> {
        match self.said(name, cwd, &|_| None)? {
            Some(said) if said.is_empty() || said.starts_with('{') => Err(Unread),
            said => Ok(said),
        }
    }

    fn said(
        &self,
        name: &str,
        cwd: &Path,
        from_file: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Option<String>, Unread> {
        let mut said: Vec<String> = Vec::new();
        for field in &self.fields {
            if let Some(value) = (field.strip_prefix(name)).and_then(|rest| rest.strip_prefix('='))
            {
                said.push(match value.strip_prefix('@') {
                    Some(file) => from_file(file).ok_or(Unread)?,
                    None => value.to_owned(),
                });
            }
        }
        if let Some(file) = &self.input {
            let text = std::fs::read_to_string(cwd.join(file)).map_err(|_| Unread)?;
            let body: serde_json::Value = serde_json::from_str(&text).map_err(|_| Unread)?;
            said.extend(body.get(name).map(|value| match value.as_str() {
                Some(value) => value.to_owned(),
                None => value.to_string(),
            }));
        }
        match said.len() {
            0 | 1 => Ok(said.pop()),
            _ => Err(Unread),
        }
    }
}

/// A field whose value the command's own words and files don't say.
struct Unread;

/// What `gh api` run with `args`, the words after `api`, calls.
pub fn github_api(args: &[impl AsRef<str>]) -> Option<ApiCall> {
    const VALUED: [(&str, &str); 10] = [
        ("method", "X"),
        ("raw-field", "f"),
        ("field", "F"),
        ("header", "H"),
        ("jq", "q"),
        ("template", "t"),
        ("preview", "p"),
        ("input", ""),
        ("hostname", ""),
        ("cache", ""),
    ];
    let (mut endpoint, mut method, mut input) = (None, None, None);
    let mut fields = Vec::new();
    let mut words = args.iter().map(AsRef::as_ref);
    while let Some(arg) = words.next() {
        let (flag, attached) = if let Some(long) = arg.strip_prefix("--") {
            match long.split_once('=') {
                Some((name, value)) => (name, Some(value)),
                None => (long, None),
            }
        } else if let Some(short) = arg.strip_prefix('-') {
            let short = short.trim_start_matches('i');
            let (letter, value) = short.split_at_checked(1).unwrap_or(("", ""));
            let name = VALUED
                .iter()
                .find(|(_, short)| !letter.is_empty() && *short == letter);
            let value = value.strip_prefix('=').unwrap_or(value);
            (
                name.map_or("", |(name, _)| *name),
                (!value.is_empty()).then_some(value),
            )
        } else {
            if endpoint.is_none() {
                endpoint = Some(plain_endpoint(arg));
            }
            continue;
        };
        if !VALUED.iter().any(|(name, _)| *name == flag) {
            continue;
        }
        let value = attached.or_else(|| words.next());
        match flag {
            "method" => method = value.map(str::to_ascii_uppercase),
            "field" | "raw-field" => fields.extend(value.map(str::to_owned)),
            "input" => input = Some(value.unwrap_or("-").to_owned()),
            _ => {}
        }
    }
    let writes = input.is_some() || !fields.is_empty();
    Some(ApiCall {
        method: method.unwrap_or_else(|| if writes { "POST" } else { "GET" }.to_owned()),
        endpoint: endpoint?,
        fields,
        input,
    })
}

/// A `gh api` endpoint as GitHub reads it: without its host, doubled slashes or escapes.
fn plain_endpoint(said: &str) -> String {
    let path = match said.split_once("://") {
        Some((_, rest)) => {
            let path = rest.split_once('/').map_or("", |(_, path)| path);
            path.strip_prefix("api/v3/").unwrap_or(path)
        }
        None => said,
    };
    let (path, query) = path
        .find(['?', '#'])
        .map_or((path, ""), |at| path.split_at(at));
    let decoded = sikemux_base::http::percent_decode(path).unwrap_or_else(|| path.to_owned());
    let parts: Vec<&str> = decoded.split('/').filter(|part| !part.is_empty()).collect();
    format!("{}{query}", parts.join("/"))
}

/// Whether `gh api` with `args` writes: a GraphQL query that holds a mutation, or a REST call
/// with a method other than GET, or with fields and no method, which gh sends as a POST.
fn api_writes(args: &[String], cwd: &Path) -> bool {
    let args: Vec<String> = gh_args(args).into_iter().skip(1).collect();
    let Some(call) = github_api(&args) else {
        return false;
    };
    if call.endpoint != "graphql" {
        return call.method != "GET";
    }
    let said = call.said("query", cwd, &|file| {
        std::fs::read_to_string(cwd.join(file)).ok()
    });
    match said {
        Ok(query) => query.is_some_and(|text| {
            text.split(|c: char| !c.is_ascii_alphanumeric())
                .any(|word| word == "mutation")
        }),
        Err(Unread) => true,
    }
}

/// Vercel's commands that read, or work on this computer only.
const VERCEL_READS: [&str; 20] = [
    "ls", "list", "logs", "inspect", "whoami", "help", "pull", "env", "domains", "dns", "certs",
    "project", "projects", "teams", "login", "logout", "switch", "link", "dev", "build",
];

/// Vercel's commands that change what is live without a deploy.
const VERCEL_TAKES_AWAY: [[&str; 2]; 20] = [
    ["env", "rm"],
    ["env", "remove"],
    ["project", "rm"],
    ["project", "remove"],
    ["projects", "rm"],
    ["projects", "remove"],
    ["domains", "rm"],
    ["domains", "remove"],
    ["domains", "move"],
    ["domains", "mv"],
    ["dns", "add"],
    ["dns", "import"],
    ["dns", "rm"],
    ["dns", "remove"],
    ["certs", "rm"],
    ["certs", "remove"],
    ["alias", "remove"],
    ["blob", "del"],
    ["blob", "delete-store"],
    ["blob", "empty-store"],
];

/// Whether Vercel's command line run with `args` changes what is live without a deploy. An
/// environment variable of preview or development only isn't live.
pub fn vercel_takes_away(args: &[impl AsRef<str>]) -> bool {
    let words: Vec<&str> = (args.iter().map(AsRef::as_ref))
        .filter(|arg| !arg.starts_with('-'))
        .collect();
    let preview = words.iter().any(|word| {
        word.eq_ignore_ascii_case("preview") || word.eq_ignore_ascii_case("development")
    }) && !words
        .iter()
        .any(|word| word.eq_ignore_ascii_case("production"));
    VERCEL_TAKES_AWAY.iter().any(|[group, command]| {
        (words.iter())
            .position(|word| word == group)
            .and_then(|at| words.get(at + 1..))
            .is_some_and(|after| after.contains(command))
            && !(*group == "env" && preview)
    })
}

/// Switches of the deploy and publish programs known to take no value, after which a
/// `--dry-run` is the program's own flag.
const SWITCHES: [&str; 33] = [
    "--install",
    "--allow-dirty",
    "--force",
    "--yes",
    "-y",
    "--wait",
    "--no-wait",
    "--atomic",
    "--prebuilt",
    "--prod",
    "--no-verify",
    "--verbose",
    "-v",
    "--quiet",
    "-q",
    "--debug",
    "--recursive",
    "--all",
    "--prune",
    "--force-with-lease",
    "--set-upstream",
    "-u",
    "--no-build",
    "--skip-build",
    "--locked",
    "--frozen",
    "--offline",
    "--no-git-checks",
    "--create-namespace",
    "--reuse-values",
    "--reset-values",
    "--cleanup-on-fail",
    "--draft",
];

const GIT_PUSH_SWITCHES: &[&str] = &[
    "--all",
    "--mirror",
    "--tags",
    "--follow-tags",
    "--atomic",
    "--no-atomic",
    "--force",
    "-f",
    "--force-with-lease",
    "--no-force-with-lease",
    "--force-if-includes",
    "--no-force-if-includes",
    "--delete",
    "-d",
    "--prune",
    "--verbose",
    "-v",
    "--quiet",
    "-q",
    "--progress",
    "--no-verify",
    "--verify",
    "--set-upstream",
    "-u",
    "--thin",
    "--no-thin",
    "--porcelain",
    "--ipv4",
    "-4",
    "--ipv6",
    "-6",
];

fn git_global_help(args: &[String]) -> bool {
    let sub = git_args(args);
    let global = args.get(..args.len() - sub.len()).unwrap_or_default();
    let mut words = global.iter();
    while let Some(arg) = words.next() {
        if arg == "--help" {
            return true;
        }
        if matches!(
            arg.as_str(),
            "-C" | "-c"
                | "--git-dir"
                | "--work-tree"
                | "--namespace"
                | "--config-env"
                | "--exec-path"
                | "--super-prefix"
        ) {
            words.next();
        }
    }
    false
}

fn has_program_flag(
    args: &[String],
    flags: &[&str],
    switches: &[&str],
    allow_dry_run_value: bool,
) -> bool {
    let mut words = args.iter();
    while let Some(arg) = words.next() {
        if arg == "--" {
            break;
        }
        if flags.contains(&arg.as_str())
            || (allow_dry_run_value
                && arg
                    .strip_prefix("--dry-run=")
                    .is_some_and(|value| value != "none"))
        {
            return true;
        }
        if arg.starts_with('-') && !arg.contains('=') && !switches.contains(&arg.as_str()) {
            words.next();
        }
    }
    false
}

fn program_skips_action(program: &str, args: &[String]) -> bool {
    match program {
        "psql" | "mysql" => matches!(args, [only] if only == "--help" || only == "--version"),
        "kubectl" | "helm" => has_program_flag(args, &["--dry-run", "--help"], &SWITCHES, true),
        "aws" => {
            if has_program_flag(args, &["--help"], &SWITCHES, false) {
                return true;
            }
            let is_s3 = args.iter().any(|arg| arg == "s3");
            is_s3 && has_program_flag(args, &["--dryrun"], &SWITCHES, false)
        }
        "npm" | "pnpm" | "yarn" | "cargo" | "uv" | "poetry" | "changeset" | "wrangler"
        | "supabase" => has_program_flag(args, &["--dry-run", "--help"], &SWITCHES, false),
        _ => has_program_flag(args, &["--help"], &SWITCHES, false),
    }
}

/// Whether a database client is given only statements that read: each passed with `-c`,
/// `--command`, `-e` or `--execute`, attached or not, with no script file or stdin redirect.
fn only_queries(args: &[String]) -> bool {
    const DB_VALUED_LONG: &[&str] = &[
        "--host",
        "--port",
        "--username",
        "--user",
        "--dbname",
        "--database",
        "--password",
        "--set",
        "--variable",
        "--pset",
        "--field-separator",
        "--record-separator",
        "--table-attr",
        "--protocol",
        "--socket",
        "--ssl-mode",
        "--defaults-file",
        "--defaults-extra-file",
        "--defaults-group-suffix",
        "--connect-timeout",
        "--default-character-set",
    ];
    const DB_VALUED_SHORT: &str = "hpUduPDSvFRT";
    const DB_FORBIDDEN_LONG: &[&str] = &[
        "--file",
        "--output",
        "--log-file",
        "--init-command",
        "--source",
    ];

    let mut given: Vec<&str> = Vec::new();
    let mut positional: Vec<&str> = Vec::new();
    let mut after_double_dash = false;
    let mut words = args.iter();
    while let Some(arg) = words.next() {
        let operator = arg.trim_start_matches(|c: char| c.is_ascii_digit());
        if operator.starts_with('<') {
            return false;
        }
        if after_double_dash {
            positional.push(arg.as_str());
            continue;
        }
        if arg == "--" {
            after_double_dash = true;
            continue;
        }
        if arg.starts_with("--") {
            let (flag, value) = match arg.split_once('=') {
                Some((flag, val)) => (flag, Some(val)),
                None => (arg.as_str(), None),
            };
            if DB_FORBIDDEN_LONG.contains(&flag) {
                return false;
            }
            if matches!(flag, "--command" | "--execute") {
                let Some(statement) = value.or_else(|| words.next().map(String::as_str)) else {
                    return false;
                };
                given.push(statement);
                continue;
            }
            if value.is_none() && DB_VALUED_LONG.contains(&flag) {
                words.next();
            }
            continue;
        }
        if let Some(short) = arg.strip_prefix('-').filter(|rest| !rest.is_empty()) {
            for (at, letter) in short.char_indices() {
                if matches!(letter, 'c' | 'e') {
                    let attached = short.get(at + letter.len_utf8()..).unwrap_or_default();
                    let statement = match attached.is_empty() {
                        true => match words.next() {
                            Some(next) => next.as_str(),
                            None => return false,
                        },
                        false => attached,
                    };
                    given.push(statement);
                    break;
                }
                if matches!(letter, 'f' | 'o' | 'L') {
                    return false;
                }
                if DB_VALUED_SHORT.contains(letter) {
                    if at + letter.len_utf8() == short.len() {
                        words.next();
                    }
                    break;
                }
                if !DB_SWITCHES.contains(letter) {
                    return false;
                }
            }
            continue;
        }
        positional.push(arg.as_str());
    }
    if positional.len() > 2
        || positional
            .iter()
            .any(|arg| arg.to_ascii_lowercase().ends_with(".sql"))
    {
        return false;
    }
    !given.is_empty() && given.iter().all(|text| statements_read(text))
}

/// psql's and mysql's short flags that take no value.
const DB_SWITCHES: &str = "aAbBEHlnNqrstwWxXz01";
/// curl's short flags that take no value.
const CURL_SWITCHES: &str = "sSvikLfgGIjJlnNOpqRZM0123456#";

/// What follows one of `flags` in a packed short flag such as `-tc"…"`, when every letter
/// before it is one of `switches`; empty when the value is the next word.
fn packed_value<'a>(short: &'a str, flags: &[char], switches: &str) -> Option<&'a str> {
    for (at, letter) in short.char_indices() {
        if flags.contains(&letter) {
            return short.get(at + letter.len_utf8()..);
        }
        if !switches.contains(letter) {
            return None;
        }
    }
    None
}

fn is_psql_listing(text: &str) -> bool {
    let trimmed = text.trim();
    let Some(rest) = trimmed.strip_prefix('\\') else {
        return false;
    };
    let starts_listing = rest.starts_with('d')
        || rest == "l"
        || rest == "l+"
        || rest.starts_with("l ")
        || rest.starts_with("l+ ");
    starts_listing && !rest.contains(['\\', ';', '|', '`', '\n', '\r'])
}

fn is_forbidden_sql_word(word: &str) -> bool {
    const FORBIDDEN: &[&str] = &[
        "insert",
        "update",
        "delete",
        "merge",
        "replace",
        "drop",
        "alter",
        "create",
        "truncate",
        "grant",
        "revoke",
        "copy",
        "vacuum",
        "call",
        "do",
        "lock",
        "load",
        "handler",
        "prepare",
        "execute",
        "deallocate",
        "set",
        "reset",
        "discard",
        "checkpoint",
        "reindex",
        "cluster",
        "refresh",
        "import",
        "attach",
        "detach",
        "into",
        "outfile",
        "dumpfile",
        "with",
        "set_config",
        "setval",
        "nextval",
        "dblink",
        "dblink_exec",
        "dblink_connect",
        "dblink_send_query",
        "load_file",
    ];
    const FORBIDDEN_PREFIXES: &[&str] = &[
        "pg_terminate_",
        "pg_cancel_",
        "pg_reload_",
        "pg_rotate_",
        "pg_switch_",
        "pg_create_",
        "pg_drop_",
        "pg_advisory_",
        "pg_try_advisory_",
        "pg_write_",
        "pg_file_",
        "pg_export_",
        "pg_import_",
        "pg_stat_reset",
        "pg_reset_",
        "pg_replication_origin_",
        "pg_logical_slot_",
        "pg_backup_",
        "pg_start_backup",
        "pg_stop_backup",
        "pg_promote",
        "pg_sleep",
        "lo_",
        "query_to_xml",
        "table_to_xml",
        "cursor_to_xml",
    ];
    FORBIDDEN.contains(&word)
        || FORBIDDEN_PREFIXES
            .iter()
            .any(|prefix| word.starts_with(prefix))
}

/// Whether every statement of `text` only reads.
fn statements_read(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }
    if is_psql_listing(trimmed) {
        return true;
    }
    let bytes = trimmed.as_bytes();
    let mut at = 0;
    let mut statements: Vec<Vec<String>> = vec![Vec::new()];
    while let Some(rest) = trimmed.get(at..).filter(|rest| !rest.is_empty()) {
        if rest.starts_with("--") {
            match rest.find('\n') {
                Some(nl) => at += nl + 1,
                None => break,
            }
            continue;
        }
        if let Some(after_open) = rest.strip_prefix("/*") {
            let Some(end) = after_open.find("*/") else {
                return false;
            };
            at += 2 + end + 2;
            continue;
        }
        let Some(&b) = bytes.get(at) else {
            break;
        };
        if b == b'\'' {
            at += 1;
            let mut closed = false;
            while let Some(&ch) = bytes.get(at) {
                if ch == b'\\' {
                    return false;
                }
                if ch == b'\'' {
                    if bytes.get(at + 1) == Some(&b'\'') {
                        at += 2;
                        continue;
                    }
                    at += 1;
                    closed = true;
                    break;
                }
                at += 1;
            }
            if !closed {
                return false;
            }
            continue;
        }
        if b == b'"' {
            at += 1;
            let mut closed = false;
            while let Some(&ch) = bytes.get(at) {
                if ch == b'"' {
                    if bytes.get(at + 1) == Some(&b'"') {
                        at += 2;
                        continue;
                    }
                    at += 1;
                    closed = true;
                    break;
                }
                at += 1;
            }
            if !closed {
                return false;
            }
            continue;
        }
        if b == b'\\' || b == b'$' {
            return false;
        }
        if b == b';' {
            statements.push(Vec::new());
            at += 1;
            continue;
        }
        if b.is_ascii_alphabetic() || b == b'_' {
            let start = at;
            while bytes
                .get(at)
                .is_some_and(|ch| ch.is_ascii_alphanumeric() || *ch == b'_')
            {
                at += 1;
            }
            if let (Some(stmt), Some(word)) = (statements.last_mut(), trimmed.get(start..at)) {
                stmt.push(word.to_ascii_lowercase());
            }
            continue;
        }
        at += rest.chars().next().map_or(1, char::len_utf8);
    }
    let non_empty: Vec<&Vec<String>> = statements.iter().filter(|stmt| !stmt.is_empty()).collect();
    if non_empty.is_empty() {
        return false;
    }
    non_empty.into_iter().all(|words| {
        let Some(first) = words.first().map(String::as_str) else {
            return false;
        };
        if !matches!(first, "select" | "show" | "describe" | "desc") {
            return false;
        }
        if words.iter().any(|word| is_forbidden_sql_word(word)) {
            return false;
        }
        let has_for_lock = words.windows(2).any(|pair| {
            matches!(
                pair,
                [a, b] if a == "for" && matches!(b.as_str(), "update" | "share" | "no" | "key")
            )
        });
        !has_for_lock
    })
}

/// The Supabase project whose database a web request runs SQL on, by its ref: one sent to the
/// Management API's query endpoint, which may write, unlike its `read-only` one.
fn supabase_sql(args: &[String]) -> Option<String> {
    web_addresses(args).into_iter().find_map(|address| {
        let segments: Vec<&str> = address.path_segments()?.filter(|s| !s.is_empty()).collect();
        match (address.host_str(), segments.as_slice()) {
            (Some("api.supabase.com"), ["v1", "projects", project, "database", "query"]) => {
                Some((*project).to_owned())
            }
            _ => None,
        }
    })
}

/// Whether every body `program`, curl or wget, sends is a SQL query that only reads, as
/// `{"query": "select …"}`; not when any body is read from a file or standard input.
fn sends_only_reads(program: &str, args: &[String]) -> bool {
    let data_flags = match program {
        "wget" => WGET_DATA,
        _ => CURL_DATA,
    };
    let given = (args.iter().enumerate()).filter_map(|(at, arg)| {
        let next = || args.get(at + 1).map(String::as_str);
        if let Some((flag, body)) = arg.split_once('=').filter(|_| arg.starts_with("--")) {
            return data_flags.contains(&flag).then_some(body);
        }
        if arg.starts_with("--") || program != "curl" {
            return data_flags.contains(&arg.as_str()).then(next).flatten();
        }
        let short = arg.strip_prefix('-')?;
        match packed_value(short, &['d', 'F', 'T'], CURL_SWITCHES)? {
            "" => next(),
            attached => Some(attached),
        }
    });
    let mut bodies = given.peekable();
    bodies.peek().is_some()
        && bodies.all(|body| {
            let query = serde_json::from_str::<serde_json::Value>(body)
                .ok()
                .and_then(|sent| sent.get("query")?.as_str().map(str::to_owned));
            query.is_some_and(|query| statements_read(&query))
        })
}

/// kubectl's and helm's own commands, and whether each changes a cluster.
const CLUSTER_VERBS: [(&str, bool); 44] = [
    ("apply", true),
    ("create", true),
    ("replace", true),
    ("edit", true),
    ("delete", true),
    ("rollout", true),
    ("set", true),
    ("scale", true),
    ("patch", true),
    ("label", true),
    ("annotate", true),
    ("taint", true),
    ("cordon", true),
    ("uncordon", true),
    ("drain", true),
    ("expose", true),
    ("run", true),
    ("autoscale", true),
    ("install", true),
    ("upgrade", true),
    ("uninstall", true),
    ("rollback", true),
    ("get", false),
    ("describe", false),
    ("logs", false),
    ("top", false),
    ("explain", false),
    ("diff", false),
    ("auth", false),
    ("config", false),
    ("version", false),
    ("api-resources", false),
    ("cluster-info", false),
    ("wait", false),
    ("plugin", false),
    ("list", false),
    ("status", false),
    ("history", false),
    ("show", false),
    ("template", false),
    ("lint", false),
    ("repo", false),
    ("search", false),
    ("dependency", false),
];

/// Whether kubectl or helm run with `args` changes the cluster: the first of their own
/// commands among the arguments is one that does, and a `rollout` restarts, undoes, pauses or
/// resumes.
fn changes_cluster(program: &str, args: &[String]) -> bool {
    let verb = args.iter().enumerate().find_map(|(at, arg)| {
        let (_, changes) = CLUSTER_VERBS.iter().find(|(verb, _)| verb == arg)?;
        Some((at, arg.as_str(), *changes))
    });
    match verb {
        Some((at, "rollout", _)) if program == "kubectl" => args
            .get(at + 1)
            .is_some_and(|what| matches!(what.as_str(), "restart" | "undo" | "pause" | "resume")),
        Some((_, _, changes)) => changes,
        None => false,
    }
}

/// The words of a `gh` command with its subcommands first, found as gh finds them: a flag
/// before them takes the word after it unless it has a value of its own.
fn gh_args(args: &[String]) -> Vec<String> {
    let (mut subcommands, mut rest) = (Vec::new(), Vec::new());
    let mut words = args.iter();
    while let Some(word) = words.next() {
        let found = subcommands.len() == 2 || subcommands.first().is_some_and(|word| word == "api");
        let short = word.starts_with('-') && word.len() == 2 && word != "-h";
        let long = word.starts_with("--") && !matches!(word.as_str(), "--help" | "--version");
        if found {
            rest.push(word.clone());
        } else if (short || long) && !word.contains('=') {
            rest.push(word.clone());
            rest.extend(words.next().cloned());
        } else if word.starts_with('-') {
            rest.push(word.clone());
        } else {
            subcommands.push(word.clone());
        }
    }
    subcommands.append(&mut rest);
    subcommands
}

const GH_RELEASE_READS: &[&str] = &["list", "ls", "view", "download", "verify", "verify-asset"];

fn gh_release_tag(rest: &[String]) -> String {
    const VALUED: &[&str] = &[
        "-R",
        "--repo",
        "-t",
        "--title",
        "-n",
        "--notes",
        "-F",
        "--notes-file",
        "--notes-start-tag",
        "--target",
        "--discussion-category",
        "--tag",
        "-H",
        "--hostname",
    ];
    const RELEASE_SWITCHES: &[&str] = &[
        "--clobber",
        "-y",
        "--yes",
        "--cleanup-tag",
        "-d",
        "--draft",
        "-p",
        "--prerelease",
        "--latest",
        "--generate-notes",
        "--verify-tag",
        "--fail-on-no-commits",
    ];
    let mut words = rest.iter();
    while let Some(word) = words.next() {
        if word == "--" {
            return words.next().cloned().unwrap_or_default();
        }
        if VALUED.contains(&word.as_str()) {
            words.next();
            continue;
        }
        if word.starts_with('-') {
            if !word.contains('=') && !RELEASE_SWITCHES.contains(&word.as_str()) {
                return String::new();
            }
            continue;
        }
        return word.clone();
    }
    String::new()
}

fn matches(rule: &Rule, args: &[String], cwd: &Path) -> bool {
    let github = rule.programs == ["gh"] && rule.words.len() == 2 && {
        let gh = gh_args(args);
        gh.len() >= rule.words.len() && gh.iter().zip(rule.words).all(|(arg, word)| arg == word)
    };
    let follows = rule.words.is_empty()
        || github
        || args
            .windows(rule.words.len())
            .any(|window| window.iter().zip(rule.words).all(|(arg, word)| arg == word));
    follows
        && match rule.needs {
            Needs::Nothing => true,
            Needs::Flag(flag) => has_flag(args, &[flag]),
            Needs::ToS3 => s3_destination(args).is_some_and(|to| to.starts_with("s3://")),
            Needs::ApiWrite => api_writes(args, cwd),
            Needs::NotQuerying => !only_queries(args),
            Needs::NotReading(reads) => args.first().is_some_and(|command| {
                !command.starts_with('-')
                    && !reads.contains(&command.as_str())
                    && !command.ends_with(":report")
            }),
        }
}

/// Which programs of a command line a check looks at.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Looks {
    Every,
    /// Only those the shim does not see, where the shim is first on PATH:
    /// programs run by a path, and the project's own commands.
    PastTheShim,
}

/// Every command of a shell command line that leaves this machine, in order,
/// as each is checked by itself. `extra` are the project's own commands that
/// do, such as `make deploy`, each matched by its leading words. `env` is what
/// the line runs under, beneath what it sets itself. A git command the line moved
/// to another folder says so with `-C`; a move the line can't spell out leaves where a
/// command lands open, and is shown in front of it.
pub fn of_command(
    line: &str,
    extra: &[String],
    looks: Looks,
    cwd: &Path,
    env: &dyn Fn(&str) -> Option<String>,
) -> Vec<Ship> {
    of_command_at(line, extra, looks, cwd, env, 0)
}

const MAX_ALIAS_DEPTH: u8 = 4;

fn of_command_at(
    line: &str,
    extra: &[String],
    looks: Looks,
    cwd: &Path,
    env: &dyn Fn(&str) -> Option<String>,
    depth: u8,
) -> Vec<Ship> {
    if depth > MAX_ALIAS_DEPTH {
        return Vec::new();
    }
    let mut folder = Folder::new(cwd);
    let mut selected: Vec<String> = Vec::new();
    let mut shim_bypassed = false;
    let mut ships = Vec::new();
    let mut line_gh_aliases: Vec<(String, String)> = Vec::new();
    let mut line_git_aliases: Vec<(String, String)> = Vec::new();
    for piece in pieces(line) {
        let all = match piece {
            Piece::Open => {
                folder.open();
                continue;
            }
            Piece::Close => {
                folder.close();
                continue;
            }
            Piece::Words(all) => all,
        };
        if tampers_with_shim(&all) {
            shim_bypassed = true;
        }
        if selects(&all) {
            selected.push(all.join(" "));
            continue;
        }
        let set_on_line = |name: &str| {
            (selected.iter().rev())
                .flat_map(|said| said.split_whitespace())
                .find_map(|word| word.strip_prefix(name)?.strip_prefix('='))
                .map(str::to_owned)
                .or_else(|| env(name))
        };
        if folder.moves(&all, &set_on_line) {
            continue;
        }
        let (words, prefix_len, unknown_flag) = match command_words(&all) {
            Unwrapped::None => continue,
            Unwrapped::Command { words, prefix_len } => (words, prefix_len, false),
            Unwrapped::UnknownWrapperFlag { prefix_len, rest } => (rest, prefix_len, true),
        };
        let set_before = all.get(..prefix_len).unwrap_or_default();
        let bypassed = shim_bypassed || unknown_flag || prefix_bypasses_shim(set_before);
        let here = match env_folder(&all) {
            Some(to) => folder.after(to, &set_on_line),
            None => folder.here.clone(),
        };
        let candidates: Vec<&[String]> = if unknown_flag {
            words
                .iter()
                .enumerate()
                .filter(|(_, word)| !word.starts_with('-'))
                .filter_map(|(at, _)| words.get(at..))
                .collect()
        } else {
            vec![words.as_ref()]
        };
        for candidate in candidates {
            if let Some(script) = shell_script(candidate) {
                let at = here.clone().unwrap_or_else(|| cwd.to_owned());
                let inner_looks = if bypassed { Looks::Every } else { looks };
                ships.extend(of_command_at(
                    script,
                    extra,
                    inner_looks,
                    &at,
                    env,
                    depth + 1,
                ));
                break;
            }
            alias::record_gh_alias(candidate, &mut line_gh_aliases);
            alias::record_git_alias(candidate, &mut line_git_aliases);
            let Some((program, raw_args)) = candidate.split_first() else {
                continue;
            };
            let set = |name: &str| {
                let said = (set_before.iter())
                    .filter_map(|word| word.strip_prefix(name)?.strip_prefix('='))
                    .next_back();
                said.map(str::to_owned).or_else(|| env(name))
            };
            let run_cwd = here.as_deref().unwrap_or(cwd);
            let mut expanded_words: Option<Vec<String>> = None;
            let mut expanded_shell: Option<(String, PathBuf)> = None;
            if depth < MAX_ALIAS_DEPTH && base_name(program) == "gh" {
                let mut aliases = alias::gh_aliases_from_config(&set);
                aliases.extend(line_gh_aliases.iter().cloned());
                match alias::expand_gh(raw_args, &aliases) {
                    Some(alias::Expanded::Shell(script)) => {
                        expanded_shell = Some((script, run_cwd.to_path_buf()));
                    }
                    Some(alias::Expanded::Args(expanded)) => {
                        let mut full = Vec::with_capacity(1 + expanded.len());
                        full.push(program.clone());
                        full.extend(expanded);
                        expanded_words = Some(full);
                    }
                    None => {}
                }
            } else if depth < MAX_ALIAS_DEPTH && base_name(program) == "git" {
                let git_lookup = |global: &[String], name: &str| {
                    let dir = alias::git_run_dir(global, run_cwd);
                    if !dir.is_dir() {
                        return None;
                    }
                    let mut cmd: Vec<&str> = global.iter().map(String::as_str).collect();
                    let key = format!("alias.{name}");
                    cmd.extend(["config", "--get", key.as_str()]);
                    crate::git::Git::quick(&dir).trimmed(&cmd).ok()
                };
                match alias::expand_git(raw_args, &line_git_aliases, &git_lookup) {
                    Some(alias::Expanded::Shell(script)) => {
                        let dir = alias::git_run_dir(raw_args, run_cwd);
                        expanded_shell = Some((script, dir));
                    }
                    Some(alias::Expanded::Args(expanded)) => {
                        let mut full = Vec::with_capacity(1 + expanded.len());
                        full.push(program.clone());
                        full.extend(expanded);
                        expanded_words = Some(full);
                    }
                    None => {}
                }
            }
            if let Some((script, at)) = expanded_shell {
                ships.extend(of_command_at(
                    &script,
                    extra,
                    Looks::Every,
                    &at,
                    &set,
                    depth + 1,
                ));
                break;
            }
            let effective = expanded_words.as_deref().unwrap_or(candidate);
            let Some((program, args)) = effective.split_first() else {
                continue;
            };
            let shimmed = !program.contains('/')
                && (program == "git" || PROGRAMS.contains(&program.as_str()));
            let known = match looks == Looks::PastTheShim && !bypassed && shimmed {
                true => None,
                false => of_program(program, args, run_cwd, &|_| None),
            };
            let Some(kind) = known.or_else(|| {
                extra
                    .iter()
                    .any(|listed| leads(listed, effective))
                    .then_some(ShipKind::Deploy)
            }) else {
                continue;
            };
            let at = here.clone().filter(|_| selected.is_empty());
            let names_its_host = matches!(base_name(program), "curl" | "wget")
                || ssh::PROGRAMS.contains(&base_name(program));
            let target = match (at, names_its_host) {
                (Some(at), _) => target_of(program, args, &set, &at),
                (None, true) => target_of(program, args, &set, cwd),
                (None, false) => None,
            };
            if !leaves_machine(program, args, target.as_ref()) {
                continue;
            }
            let git = base_name(program) == "git";
            let moved: Vec<String> = (here.iter())
                .filter(|here| git && *here != cwd)
                .flat_map(|here| ["-C".to_owned(), here.to_string_lossy().into_owned()])
                .chain(args.iter().cloned())
                .collect();
            let command = match moved.len() == args.len() {
                true => effective.join(" "),
                false => format!("{program} {}", moved.join(" ")),
            };
            let configured = configured(program, &moved, &set);
            let what: Vec<String> = (configured.into_iter())
                .chain(
                    (selected.iter())
                        .filter(|said| !git || said.contains("GIT_"))
                        .cloned(),
                )
                .chain(here.is_none().then(|| folder.unread.join(" && ")))
                .chain(std::iter::once(command))
                .collect();
            ships.push(Ship {
                kind,
                what: what.join(" && "),
                target,
            });
            break;
        }
    }
    ships
}

/// The folder each command of a shell line runs in, as `cd`, `pushd` and `popd` move it, and
/// a subshell's moves end with the subshell. None once a move the line can't spell out happened.
struct Folder {
    here: Option<PathBuf>,
    /// The moves that could not be read, as written.
    unread: Vec<String>,
    pushed: Vec<Option<PathBuf>>,
    outer: Vec<(Option<PathBuf>, usize, usize)>,
}

impl Folder {
    fn new(cwd: &Path) -> Self {
        Self {
            here: Some(cwd.to_owned()),
            unread: Vec::new(),
            pushed: Vec::new(),
            outer: Vec::new(),
        }
    }

    fn open(&mut self) {
        (self.outer).push((self.here.clone(), self.unread.len(), self.pushed.len()));
    }

    fn close(&mut self) {
        if let Some((here, unread, pushed)) = self.outer.pop() {
            self.here = here;
            self.unread.truncate(unread);
            self.pushed.truncate(pushed);
        }
    }

    /// Follows `all` when it is a change of folder, saying whether it was.
    fn moves(&mut self, all: &[String], set: &dyn Fn(&str) -> Option<String>) -> bool {
        let Some((verb, rest)) = all.split_first() else {
            return false;
        };
        let to = match (verb.as_str(), rest) {
            ("cd" | "pushd", []) => set("HOME").map(PathBuf::from),
            ("cd" | "pushd", [to]) if to != "-" => self.after(to, set),
            ("popd", []) => self.pushed.pop().flatten(),
            ("cd" | "pushd" | "popd", _) => None,
            _ => return false,
        };
        if verb == "pushd" {
            self.pushed.push(self.here.clone());
        }
        if to.is_none() {
            self.unread.push(all.join(" "));
        }
        self.here = to;
        true
    }

    /// Where moving to `to` from here lands, its `~` and `$NAME`s read through `set`.
    fn after(&self, to: &str, set: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
        let to = match to.strip_prefix('~') {
            Some(rest) => format!("{}{rest}", set("HOME")?),
            None => to.to_owned(),
        };
        let to = PathBuf::from(expanded(&to, set)?);
        match to.is_absolute() {
            true => Some(to),
            false => Some(self.here.as_ref()?.join(to)),
        }
    }
}

/// `text` with each `$NAME` and `${NAME}` read through `set`; none when one is not set, or
/// the text runs a command.
fn expanded(text: &str, set: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    if text.contains('`') || text.contains("$(") {
        return None;
    }
    let mut out = String::new();
    let mut rest = text;
    while let Some((before, after)) = rest.split_once('$') {
        out.push_str(before);
        let (name, next) = match after.strip_prefix('{') {
            Some(braced) => braced.split_once('}')?,
            None => {
                let end = (after.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')))
                    .unwrap_or(after.len());
                after.split_at(end)
            }
        };
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return None;
        }
        let value = set(name)?;
        if value.contains('$') || value.contains('`') {
            return None;
        }
        out.push_str(&value);
        rest = next;
    }
    out.push_str(rest);
    Some(out)
}

fn is_shim_var(name: &str) -> bool {
    matches!(
        name,
        "PATH"
            | "SIKEMUX_CLI_ENDPOINT"
            | "SIKEMUX_CONFIG_DIR"
            | "SIKEMUX_CLI_ENDPOINT_PUBLISH"
            | "SIKEMUX_REAL_GIT"
    )
}

fn assigns_shim_var(word: &str) -> bool {
    word.split_once('=')
        .is_some_and(|(name, _)| is_shim_var(name.trim_end_matches('+')))
}

fn tampers_with_shim(words: &[String]) -> bool {
    let Some((first, rest)) = words.split_first() else {
        return false;
    };
    if matches!(first.as_str(), "unset" | "hash" | "alias") {
        return true;
    }
    if matches!(first.as_str(), "export" | "declare" | "typeset" | "local") {
        return rest.iter().any(|word| {
            assigns_shim_var(word)
                || is_shim_var(word)
                || (word.starts_with('-') && !word.starts_with("--") && word.contains('n'))
        });
    }
    words
        .iter()
        .take_while(|word| word.contains('=') && !word.starts_with('-'))
        .any(|word| assigns_shim_var(word))
}

fn prefix_bypasses_shim(prefix: &[String]) -> bool {
    let mut runner: Option<&str> = None;
    for word in prefix {
        if assigns_shim_var(word) {
            return true;
        }
        let base = base_name(word);
        match base {
            "sudo" => return true,
            "env" | "command" | "exec" | "nice" | "time" | "nohup" | "builtin" => {
                runner = Some(base);
                continue;
            }
            _ => {}
        }
        match runner {
            Some("env")
                if word == "-"
                    || word == "--ignore-environment"
                    || word == "--unset"
                    || word.starts_with("--unset=")
                    || word == "--split-string"
                    || word.starts_with("--split-string=")
                    || (word.starts_with('-')
                        && !word.starts_with("--")
                        && word.contains(['i', 'u', 'S', 'P'])) =>
            {
                return true;
            }
            Some("command")
                if word.starts_with('-') && !word.starts_with("--") && word.contains('p') =>
            {
                return true;
            }
            Some("exec")
                if word.starts_with('-') && !word.starts_with("--") && word.contains('c') =>
            {
                return true;
            }
            _ => {}
        }
    }
    false
}

/// Whether a command of a line changes where later commands of that line land: a variable
/// set by itself or exported, or a tool told to use another context, profile, workspace or
/// stack.
fn selects(words: &[String]) -> bool {
    let said = |want: &[&str]| {
        words.len() >= want.len() && words.iter().zip(want).all(|(word, want)| word == want)
    };
    let assigns = !words.is_empty()
        && words
            .iter()
            .all(|word| word.contains('=') && !word.starts_with('-'));
    assigns
        || said(&["export"])
        || said(&["set"])
        || said(&["kubectl", "config", "use-context"])
        || said(&["kubectl", "config", "set-context"])
        || said(&["terraform", "workspace", "select"])
        || said(&["terraform", "workspace", "new"])
        || said(&["pulumi", "stack", "select"])
        || said(&["aws", "configure"])
        || said(&["gcloud", "config", "set"])
        || said(&["fly", "apps", "open"])
}

/// What `program` run with `args` from `cwd` under `env` does that leaves this machine, as the
/// stand-in on agents' PATH sees it run, whether typed or started by a script, a make target or
/// a package script.
pub(crate) fn of_invocations(
    program: &str,
    args: &[String],
    env: &dyn Fn(&str) -> Option<String>,
    cwd: &Path,
) -> Vec<Ship> {
    of_invocations_at(program, args, env, cwd, 0)
}

fn of_invocations_at(
    program: &str,
    args: &[String],
    env: &dyn Fn(&str) -> Option<String>,
    cwd: &Path,
    depth: u8,
) -> Vec<Ship> {
    if depth < MAX_ALIAS_DEPTH && program_name(program) == "gh" {
        let aliases = alias::gh_aliases_from_config(env);
        match alias::expand_gh(args, &aliases) {
            Some(alias::Expanded::Shell(script)) => {
                return of_command_at(&script, &[], Looks::Every, cwd, env, depth + 1);
            }
            Some(alias::Expanded::Args(expanded)) => {
                return of_invocations_at(program, &expanded, env, cwd, depth + 1);
            }
            None => {}
        }
    }
    let Some(kind) = of_program(program, args, cwd, &|_| None) else {
        return Vec::new();
    };
    let target = target_of(program, args, env, cwd);
    leaves_machine(program, args, target.as_ref())
        .then(|| Ship {
            kind,
            target,
            what: (configured(program, args, env).into_iter())
                .chain(std::iter::once(program.to_owned()))
                .chain(args.iter().cloned())
                .collect::<Vec<_>>()
                .join(" "),
        })
        .into_iter()
        .collect()
}

/// What `git` run with `args` from `cwd` under `env` does that leaves this machine, expanding
/// git aliases (including `-c alias.*` and `!` shell aliases) to what git really runs.
pub(crate) fn of_git_invocation(
    args: &[String],
    env: &dyn Fn(&str) -> Option<String>,
    cwd: &Path,
    lookup: &dyn Fn(&[String], &str) -> Option<String>,
) -> Vec<Ship> {
    of_git_invocation_at(args, env, cwd, lookup, 0)
}

fn of_git_invocation_at(
    args: &[String],
    env: &dyn Fn(&str) -> Option<String>,
    cwd: &Path,
    lookup: &dyn Fn(&[String], &str) -> Option<String>,
    depth: u8,
) -> Vec<Ship> {
    if depth < MAX_ALIAS_DEPTH {
        match alias::expand_git(args, &[], lookup) {
            Some(alias::Expanded::Shell(script)) => {
                let dir = alias::git_run_dir(args, cwd);
                return of_command_at(&script, &[], Looks::Every, &dir, env, depth + 1);
            }
            Some(alias::Expanded::Args(expanded)) => {
                return of_git_invocation_at(&expanded, env, cwd, lookup, depth + 1);
            }
            None => {}
        }
    }
    let Some(kind) = of_program("git", args, cwd, &|_| None) else {
        return Vec::new();
    };
    vec![Ship {
        kind,
        what: (configured("git", args, env).into_iter())
            .chain(std::iter::once("git".to_owned()))
            .chain(args.iter().cloned())
            .collect::<Vec<_>>()
            .join(" "),
        target: None,
    }]
}

#[cfg(test)]
pub(crate) fn of_invocation(
    program: &str,
    args: &[String],
    env: &dyn Fn(&str) -> Option<String>,
    cwd: &Path,
) -> Option<Ship> {
    of_invocations(program, args, env, cwd).into_iter().next()
}

/// What a push or merge is given that its words, split at spaces, don't say, as the words that
/// put it in front of the command.
pub(crate) fn configured(
    program: &str,
    args: &[String],
    env: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    let merges = || gh_args(args).starts_with(&["pr".to_owned(), "merge".to_owned()]);
    match base_name(program) {
        "git" => git_configured(env).or_else(|| {
            (args.iter().any(|arg| arg.contains(char::is_whitespace))).then(|| "…".to_owned())
        }),
        "gh" if merges() => ["GH_REPO", "GH_HOST"]
            .into_iter()
            .find_map(|name| Some(format!("{name}={}", env(name)?))),
        _ => None,
    }
}

/// Git settings given through the environment, which the push judge can't read.
fn git_configured(env: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    let set = [
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_PARAMETERS",
        "GIT_CONFIG",
        "GIT_CONFIG_GLOBAL",
        "GIT_CONFIG_SYSTEM",
        "GIT_DIR",
        "GIT_COMMON_DIR",
        "GIT_NAMESPACE",
    ]
    .into_iter()
    .find(|name| env(name).is_some());
    let own_home = own_home();
    let home = || {
        let home = env("HOME")?;
        (Path::new(&home) != own_home.as_deref()?).then_some("HOME")
    };
    let config_home = || {
        let config = PathBuf::from(env("XDG_CONFIG_HOME")?);
        let own = own_home.as_deref()?.join(".config");
        (config != own && config.join("git/config").exists()).then_some("XDG_CONFIG_HOME")
    };
    set.or_else(home)
        .or_else(config_home)
        .map(|name| format!("{name}=…"))
}

/// The home this computer's accounts give the user, whatever `HOME` says.
#[cfg(unix)]
fn own_home() -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    static HOME: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    // SAFETY: getpwuid's record is read once, and its home is copied out at once.
    let read = || unsafe {
        let user = libc::getpwuid(libc::getuid());
        if user.is_null() || (*user).pw_dir.is_null() {
            return None;
        }
        let home = std::ffi::CStr::from_ptr((*user).pw_dir);
        Some(std::ffi::OsStr::from_bytes(home.to_bytes()).into())
    };
    HOME.get_or_init(read).clone()
}

#[cfg(not(unix))]
fn own_home() -> Option<PathBuf> {
    None
}

/// Whether `words` begin with the words of `listed`, a path such as
/// `./scripts/deploy.sh` matching one written `scripts/deploy.sh`.
pub fn leads(listed: &str, words: &[String]) -> bool {
    let plain = |word: &str| word.trim_start_matches("./").to_owned();
    let listed: Vec<String> = listed.split_whitespace().map(plain).collect();
    !listed.is_empty()
        && words.len() >= listed.len()
        && words
            .iter()
            .zip(&listed)
            .all(|(word, want)| plain(word) == *want)
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

/// The registry a GitHub release goes to.
pub const GITHUB_RELEASES: &str = "GitHub releases";
/// The tool of a [`Target::Stage`] that is another computer, by its host name.
pub const SSH: &str = "SSH";
/// Words in a name that say it is production.
const PRODUCTION_WORDS: [&str; 4] = ["prod", "production", "prd", "live"];
/// Words in a name that say it is clearly not production.
const NOT_PRODUCTION_WORDS: [&str; 13] = [
    "dev",
    "develop",
    "development",
    "staging",
    "stage",
    "stg",
    "qa",
    "uat",
    "test",
    "testing",
    "sandbox",
    "preview",
    "demo",
];

pub fn says_production(name: &str) -> bool {
    has_word(name, &PRODUCTION_WORDS)
}

pub fn says_not_production(name: &str) -> bool {
    !says_production(name) && has_word(name, &NOT_PRODUCTION_WORDS)
}

fn has_word(name: &str, words: &[&str]) -> bool {
    name.split(|c: char| !c.is_ascii_alphanumeric())
        .any(|word| words.iter().any(|said| word.eq_ignore_ascii_case(said)))
}

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
        let keyed = [
            ("AWS_PROFILE", "dev"),
            ("AWS_ACCESS_KEY_ID", "AKIAEXAMPLE1234"),
        ];
        let keys = Target::AwsProfile {
            name: "access key ending 1234".into(),
        };
        let found = target("aws", "ecs update-service", &keyed, cwd);
        assert_eq!(found.as_ref().map(Target::bare), Some(&keys));
        let default = target("aws", "ecs update-service", &[], cwd);
        assert_eq!(
            default.as_ref().and_then(Target::resolved_from),
            Some("the default AWS profile")
        );
    }

    #[test]
    fn terraform_and_aws_read_the_selectors_they_are_given() {
        let folder = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(folder.path().join("data")).unwrap();
        std::fs::write(folder.path().join("data/environment"), "prod-eu").unwrap();
        let moved = target(
            "terraform",
            "apply",
            &[("TF_DATA_DIR", "data")],
            folder.path(),
        );
        assert_eq!(
            moved.as_ref().map(Target::bare),
            Some(&Target::TerraformWorkspace {
                name: "prod-eu".into()
            })
        );
        let cwd = Path::new("/nowhere");
        assert_eq!(
            target(
                "aws",
                "ecs update-service",
                &[("AWS_DEFAULT_PROFILE", "dev")],
                cwd
            ),
            Some(Target::AwsProfile { name: "dev".into() })
        );
    }

    #[test]
    fn terraform_says_where_from_its_workspace() {
        let cwd = Path::new("/nowhere");
        let default = target("terraform", "apply", &[], cwd);
        assert_eq!(
            default.as_ref().map(Target::bare),
            Some(&Target::TerraformWorkspace {
                name: "default".into()
            })
        );
        assert_eq!(
            target("terraform", "apply", &[("TF_WORKSPACE", "dev")], cwd),
            Some(Target::TerraformWorkspace { name: "dev".into() })
        );
    }

    /// The stand-in on agents' PATH is what a script, a make target or a package script meets
    /// when it starts a deploy program, and it judges the program as the hook judges it typed.
    #[test]
    fn a_program_a_script_starts_is_judged_as_when_typed() {
        let cwd = Path::new("/nowhere");
        let env = |name: &str| (name == "AWS_PROFILE").then(|| "acme-prod".to_owned());
        for line in [
            "vercel --prod",
            "vercel deploy --target production",
            "fly deploy -a acme",
            "npm publish",
            "npx vercel --prod",
            "kubectl --context prod apply -f k8s",
            "gh release create v1.0.0",
            "docker push ghcr.io/acme/web",
            "aws ecs update-service --service web",
            "wrangler deploy --env production",
        ] {
            let typed = of_command(line, &[], Looks::Every, cwd, &env);
            let words = words(line);
            let started = of_invocation(&words[0], &words[1..], &env, cwd);
            assert_eq!(typed, started.into_iter().collect::<Vec<_>>(), "{line}");
            assert_eq!(typed.len(), 1, "{line}");
        }
    }

    /// A project's own script sent a DELETE and an UPDATE to its production Supabase this way
    /// on 2026-11-23, its SQL piped in, and the gate let both through as a write to a website.
    #[test]
    fn sql_sent_to_a_supabase_project_is_a_production_database_write() {
        let cwd = Path::new("/nowhere");
        let query = "https://api.supabase.com/v1/projects/abcdefghijklmnopqrst/database/query";
        let args = |rest: &[&str]| -> Vec<String> {
            [
                "-sS",
                "-X",
                "POST",
                query,
                "-H",
                "Authorization: Bearer token",
            ]
            .iter()
            .chain(rest)
            .map(|arg| (*arg).to_owned())
            .collect()
        };
        let piped =
            of_invocation("curl", &args(&["--data-binary", "@-"]), &|_| None, cwd).expect("held");
        assert_eq!(piped.kind, ShipKind::ProductionWrite);
        assert_eq!(
            piped.target,
            Some(Target::Database {
                name: "Supabase project abcdefghijklmnopqrst".into()
            })
        );
        let update = args(&[
            "--data",
            r#"{"query":"select 1; update templates set x = 1"}"#,
        ]);
        assert!(of_invocation("curl", &update, &|_| None, cwd).is_some());
        let select = args(&["--data", r#"{"query":"select * from templates"}"#]);
        assert_eq!(of_invocation("curl", &select, &|_| None, cwd), None);
        let read_only = format!("{query}/read-only");
        let reads = ["-X", "POST", &read_only, "--data-binary", "@-"].map(str::to_owned);
        let read = of_invocation("curl", &reads, &|_| None, cwd).expect("a web write");
        assert_eq!(read.kind, ShipKind::Publish);
    }

    #[test]
    fn github_and_vercel_changes_are_seen_however_spelled() {
        let cwd = Path::new("/nowhere");
        let seen = |line: &str| {
            let words = words(line);
            of_invocation(&words[0], &words[1..], &|_| None, cwd)
        };
        for line in [
            "gh pr -R acme/site merge 5",
            "gh -R acme/site pr merge 5",
            "gh pr --squash=true merge 5",
            "gh pr --body x merge 5",
        ] {
            assert_eq!(
                seen(line).map(|ship| ship.kind),
                Some(ShipKind::Merge),
                "{line}"
            );
        }
        for line in [
            "vercel env rm KEY production",
            "vercel project rm site",
            "vercel dns rm rec_1",
            "vercel rm dpl_A",
            "vercel alias remove acme.dev",
            "vercel env --debug rm KEY production",
            "vc rm dpl_A",
        ] {
            assert_eq!(
                seen(line).map(|ship| ship.kind),
                Some(ShipKind::Deploy),
                "{line}"
            );
        }
        for line in [
            "vercel env ls",
            "vercel env rm KEY preview",
            "vercel project ls",
            "vercel dns ls",
            "gh pr list --search merge",
            "gh pr view 5 -R acme/site",
        ] {
            assert_eq!(seen(line), None, "{line}");
        }
        let lands = |line: &str| match seen(line).and_then(|ship| ship.target) {
            Some(Target::GitHub { lands, .. }) => lands,
            _ => None,
        };
        let branch = |name: &str| {
            Some(Lands::Branch {
                name: name.to_owned(),
            })
        };
        let api = "gh api repos/acme/site";
        assert_eq!(
            lands(&format!("{api}/pulls/5/merge -X PUT")),
            Some(Lands::Merge { pull: "5".into() })
        );
        assert_eq!(
            lands(&format!("{api}/git/refs/heads/main -X PATCH -f sha=a")),
            branch("main")
        );
        assert_eq!(
            lands(&format!("{api}/git/refs -f ref=refs/tags/v1 -f sha=a")),
            Some(Lands::Tag { name: "v1".into() })
        );
        assert_eq!(
            lands(&format!("{api}/contents/a.txt -X PUT -f branch=pr-assets")),
            branch("pr-assets")
        );
        assert_eq!(
            lands(&format!("{api}/contents/a.txt -X PUT -f content=a")),
            branch("HEAD")
        );
        assert_eq!(
            lands(&format!("{api}/contents/a.txt -X PUT --input -")),
            Some(Lands::Unread)
        );
        let merge = Some(Lands::Merge { pull: "5".into() });
        for line in [
            "gh api -XPUT repos//acme/site/pulls/5/merge",
            "gh -X PUT api repos/acme/site/%70ulls/5/merge",
            "gh api https://api.github.com/repos/acme/site/pulls/5/merge --method=PUT",
            "gh api repositories/7/pulls/5/merge -X PUT",
        ] {
            assert_eq!(lands(line), merge, "{line}");
        }
        assert_eq!(
            lands("gh api graphql -f query=mutation{mergePullRequest(input:{})}"),
            Some(Lands::Any)
        );
        assert_eq!(
            lands(&format!("{api}/contents/a.txt -XPUT -fbranch=pr-assets")),
            branch("pr-assets")
        );
        assert_eq!(
            lands(&format!("{api}/releases -f tag_name=v1")),
            Some(Lands::Release { tag: "v1".into() })
        );
        assert_eq!(
            lands(&format!("{api}/releases -f tag_name=v1 -F draft=true")),
            None
        );
        assert_eq!(lands(&format!("{api}/issues/5/comments -f body=a")), None);
    }

    fn over_ssh(line: &str) -> Vec<Ship> {
        of_command(line, &[], Looks::Every, Path::new("/nowhere"), &|_| None)
    }

    /// An agent stopped a service, deleted its files and ran the deploy script on a production
    /// server over SSH on 2026-11-25, and the gate saw none of it.
    #[test]
    fn a_change_made_over_ssh_lands_on_its_host() {
        for line in [
            "ssh -o BatchMode=yes citadel 'set -e\nsystemctl disable --now web.timer\n\
             rm -f /etc/systemd/system/web.timer\nsystemctl daemon-reload\n\
             systemctl is-active api 2>/dev/null||true'",
            "set -e; D=$(mktemp -d); tar -C \"$D/server/deploy\" -cf - . | ssh -o BatchMode=yes \
             citadel 'set -e; d=/root/deploy; mkdir $d; tar -xf - -C $d; $d/setup-citadel.sh $k \
             2>&1 | tail -30; cat /srv/current/DEPLOY'",
            "ssh citadel 'cd /root/deploy; . ./lib.sh; find . -name \"._*\" -delete'",
            "ssh -fp 22 root@citadel systemctl restart web",
            "ssh citadel -p 22 'echo hi > /etc/motd'",
            "ssh citadel 'echo \"$(rm -rf /srv/old)\"'",
            "ssh citadel 'sudo -u postgres psql -c \"delete from users\"'",
            "ssh citadel 'curl -sX POST http://127.0.0.1:4000/reset'",
            "ssh citadel 'bash -s' < setup.sh",
            "ssh citadel < setup.sh",
            "scp -P 22 build.tar root@citadel:/srv/",
            "rsync -av --delete -e 'ssh -p 22' dist/ citadel:/srv/web/",
            "sftp -b upload.txt citadel",
        ] {
            let ships = over_ssh(line);
            let landed: Vec<_> = (ships.iter())
                .map(|ship| (ship.kind, ship.target.clone()))
                .collect();
            assert_eq!(
                landed,
                [(ShipKind::Deploy, Some(stage(SSH, "citadel")))],
                "{line}"
            );
        }
        let open = over_ssh("ssh $HOST reboot");
        assert_eq!(open.first().map(|ship| ship.target.clone()), Some(None));
    }

    #[test]
    fn what_only_reads_over_ssh_changes_nothing() {
        for line in [
            "ssh -o BatchMode=yes citadel 'date; whoami; hostname; systemctl list-timers \
             --no-pager; ls -la /etc/sikemux 2>&1 | grep -i -E \"leader|sikemux\"; cat \
             /usr/local/lib/sikemux/deploy.hash'",
            "ssh citadel 'for f in a b; do cat $f; done'",
            "ssh citadel 'curl -fsS -m5 http://127.0.0.1:4000/v1/health >/dev/null 2>&1 || echo down'",
            "ssh citadel \"sudo -u postgres psql -Atc 'select count(*) from users where id > 5'\"",
            "ssh citadel 'journalctl -u web -n 50; docker compose logs web; find / -name x 2>/dev/null'",
            "ssh citadel 'git -C /srv/app status; git log -n 5'",
            "ssh citadel uptime > up.txt",
            "ssh -fNL 8080:localhost:80 citadel",
            "ssh -G citadel",
            "ssh git@github.com 'git-upload-pack org/repo.git'",
            "ssh localhost reboot",
            "scp citadel:/var/log/web.log .",
            "rsync -avn dist/ citadel:/srv/web/",
            "rsync --dry-run -av dist/ citadel:/srv/web/",
            "rsync -a citadel:/srv/web/ backup/",
        ] {
            assert_eq!(over_ssh(line), [], "{line}");
        }
    }

    #[test]
    fn past_the_shim_does_not_skip_env_or_path_bypasses() {
        let cwd = Path::new("/nowhere");
        let past = |line: &str| of_command(line, &[], Looks::PastTheShim, cwd, &|_| None);
        assert_eq!(past("vercel --prod"), []);
        assert_eq!(past("FOO=bar vercel --prod"), []);
        for line in [
            "env -u SIKEMUX_CLI_ENDPOINT vercel --prod",
            "env -i vercel --prod",
            "env - vercel --prod",
            "env -S 'vercel --prod'",
            "env -S '-u SIKEMUX_CLI_ENDPOINT vercel --prod'",
            "PATH=/usr/bin:/bin vercel --prod",
            "export PATH=/usr/bin:/bin; vercel --prod",
            "PATH=/usr/bin:/bin; vercel --prod",
            "unset SIKEMUX_CLI_ENDPOINT && vercel --prod",
            "unset PATH; vercel --prod",
            "command -p vercel --prod",
            "command -p git push origin main",
            "sudo -u root vercel --prod",
            "exec -c vercel --prod",
            "PATH=/usr/bin:/bin sh -c 'vercel --prod'",
            "unset SIKEMUX_CLI_ENDPOINT; bash -c 'git push origin main'",
        ] {
            assert!(
                !past(line).is_empty(),
                "expected held under PastTheShim: {line}"
            );
        }
    }

    #[test]
    fn wrapper_flags_and_dry_run_smuggling_do_not_bypass_gate() {
        let cwd = Path::new("/nowhere");
        let every = |line: &str| of_command(line, &[], Looks::Every, cwd, &|_| None);
        for line in [
            "command -p vercel --prod",
            "sudo -u root vercel --prod",
            "sudo -Eu root -g wheel vercel --prod",
            "sudo -C 3 -h prod vercel --prod",
            "nice -n 10 vercel --prod",
            "nice -10 vercel --prod",
            "time -p vercel --prod",
            "env -S 'vercel --prod'",
            "env --split-string='vercel --prod'",
            "sudo --unknown-flag vercel --prod",
            "ssh prod 'rm -rf / #' --dry-run",
            "ssh prod 'rm -rf /' --help",
            "curl -d '--dry-run' -X DELETE https://api.example.com/prod",
            "curl -X DELETE https://api.example.com/prod --dry-run",
            "curl -X DELETE https://api.example.com/prod --help",
            "psql -h prod.example.com -c 'DROP TABLE users' --dry-run",
            "psql -h prod.example.com -c 'DROP TABLE users' --help",
            "vercel --prod --dry-run",
            "kubectl delete pod web -- --dry-run",
            "kubectl apply -f --dry-run",
            "git push origin -- -n",
            "git push -o --dry-run origin main",
            "rsync -e 'ssh -n' -av dist/ citadel:/srv/web/",
            "rsync -av dist/ citadel:/srv/web/ -- -n",
        ] {
            assert!(!every(line).is_empty(), "expected held: {line}");
        }
        for line in [
            "git push --dry-run origin main",
            "git push -n origin main",
            "git push --force-with-lease --dry-run origin main",
            "git --help push",
            "kubectl apply -f pod.yaml --dry-run=client",
            "helm upgrade app ./chart --dry-run",
            "aws s3 sync dist/ s3://prod-bucket --dryrun",
            "npm publish --dry-run",
            "cargo publish --allow-dirty --dry-run",
            "vercel --help",
            "command -v vercel",
            "psql --help",
        ] {
            assert_eq!(every(line), [], "expected allowed: {line}");
        }
        let upper = of_invocation(
            "vercel",
            &words("deploy --target PRODUCTION"),
            &|_| None,
            cwd,
        )
        .expect("held");
        assert_eq!(upper.target, Some(Target::Vercel { production: true }));
    }

    #[test]
    fn ssh_read_only_classifier_blocks_rce_bypasses() {
        for line in [
            "ssh git@prod.example.com 'rm -rf /'",
            "ssh -T git@prod.example.com",
            "ssh git@prod.example.com 'git-receive-pack org/repo.git'",
            "ssh prod 'echo \"$( (echo ok); rm -rf / )\"'",
            "ssh prod 'echo $(rm -rf /)'",
            "ssh prod 'echo `rm -rf /`'",
            "ssh prod 'cat <(rm -rf /)'",
            "ssh prod 'tee >(sh)'",
            "ssh prod 'sed -n 1e /etc/passwd'",
            "ssh prod 'sed \"w /tmp/pwn\" /etc/passwd'",
            "ssh prod 'awk \"BEGIN { system (\\\"rm -rf /\\\") }\"'",
            "ssh prod 'awk \"BEGIN { print 1 > \\\"/tmp/pwn\\\" }\"'",
            "ssh prod 'awk \"BEGIN { \\\"id\\\" | getline }\"'",
            "ssh prod 'git -c core.fsmonitor=/tmp/pwn status'",
            "ssh prod 'git -ccore.fsmonitor=/tmp/pwn status'",
            "ssh prod 'git --config-env=core.fsmonitor=X status'",
            "ssh prod 'git --exec-path=/tmp status'",
            "ssh prod 'git --upload-pack=/tmp/pwn status'",
            "ssh prod 'git diff --ext-diff'",
            "ssh prod 'xxd -r dump /tmp/pwn'",
            "ssh prod 'echo pwn >\"/tmp/a b\"'",
            "ssh prod 'echo pwn >>\"/tmp/a b\"'",
            "ssh prod 'echo pwn >/dev/null.evil'",
            "ssh prod 'sudo --unknown-flag id'",
            "ssh prod 'sudo -i'",
            "ssh prod 'sudo -s id'",
        ] {
            assert!(!over_ssh(line).is_empty(), "expected SSH held: {line}");
        }
    }

    #[test]
    fn sql_read_only_gate_blocks_dml_and_script_bypasses() {
        let cwd = Path::new("/nowhere");
        let env = |_: &str| None;
        let query = "https://api.supabase.com/v1/projects/abcdefghijklmnopqrst/database/query";
        for line in [
            "psql -h prod.example.com -c 'EXPLAIN ANALYZE DELETE FROM users'",
            "psql -h prod.example.com -c 'EXPLAIN (ANALYZE) UPDATE users SET a = 1'",
            "psql -h prod.example.com -c 'EXPLAIN SELECT 1'",
            "psql -h prod.example.com -c \"SELECT 'DROP TABLE users' \\gexec\"",
            "psql -h prod.example.com -c 'SELECT 1 \\! rm -rf /'",
            "psql -h prod.example.com -c 'SELECT 1 \\o /tmp/pwn'",
            "psql -h prod.example.com -c 'SELECT 1 \\copy users TO PROGRAM id'",
            "psql -h prod.example.com -c \"SELECT * INTO OUTFILE '/tmp/pwn' FROM users\"",
            "psql -h prod.example.com -c 'SELECT * FROM users FOR UPDATE'",
            "psql -h prod.example.com -c 'SELECT * FROM users FOR SHARE'",
            "psql -h prod.example.com -c 'WITH d AS (DELETE FROM users RETURNING *) SELECT * FROM d'",
            "psql -h prod.example.com -c 'SELECT * FROM (WITH d AS (DELETE FROM users RETURNING *) SELECT * FROM d) t'",
            "psql -h prod.example.com -c 'SELECT pg_terminate_backend(123)'",
            "psql -h prod.example.com -c \"SELECT set_config('a', 'b', false)\"",
            "psql -h prod.example.com -c \"SELECT lo_export(1, '/tmp/pwn')\"",
            "psql -h prod.example.com -c 'SELECT 1' -f drop_prod.sql",
            "psql -h prod.example.com -c 'SELECT 1' --file=drop_prod.sql",
            "psql -h prod.example.com -c 'SELECT 1' -tf drop_prod.sql",
            "psql -h prod.example.com -c 'SELECT 1' < drop_prod.sql",
            "psql -h prod.example.com -c '\\dt; \\gexec'",
            "psql -h prod.example.com -c ''",
            "mysql -h prod.example.com -e 'SELECT 1' --init-command='DROP TABLE users'",
        ] {
            let ships = of_command(line, &[], Looks::Every, cwd, &env);
            assert!(!ships.is_empty(), "expected SQL held: {line}");
        }
        for sql in [
            "EXPLAIN ANALYZE DELETE FROM users",
            "SELECT * FROM users FOR UPDATE",
            "SELECT pg_terminate_backend(pid) FROM pg_stat_activity",
            "SELECT 'DROP TABLE users' \\gexec",
        ] {
            let body = serde_json::json!({ "query": sql }).to_string();
            let args = vec![
                "-X".to_owned(),
                "POST".to_owned(),
                query.to_owned(),
                "--data".to_owned(),
                body,
            ];
            let held = of_invocation("curl", &args, &env, cwd);
            assert!(held.is_some(), "expected Supabase SQL held: {sql}");
        }
        for line in [
            "psql -h prod.example.com -Atc \"SELECT id, name FROM users WHERE status = 'deleted' LIMIT 5\"",
            "psql -h prod.example.com -c '\\dt'",
            "psql -h prod.example.com -c '\\l'",
            "mysql -h prod.example.com -e 'SHOW TABLES'",
            "mysql -h prod.example.com -e 'DESCRIBE users'",
        ] {
            let ships = of_command(line, &[], Looks::Every, cwd, &env);
            assert_eq!(ships, [], "expected read-only SQL allowed: {line}");
        }
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
    /// A plain `cd` is followed since 2026-11-12, so a tool's own config is read in the folder
    /// it runs in; one the line can't spell out still leaves where it lands open.
    fn a_change_of_folder_the_line_cannot_read_leaves_where_it_lands_open() {
        let cwd = Path::new("/nowhere");
        let line = "cd $INFRA && TF_WORKSPACE=dev terraform apply";
        let ships = of_command(line, &[], Looks::Every, cwd, &|_| None);
        assert_eq!(ships.len(), 1);
        assert_eq!(ships[0].target, None);
        let line = "cd infra && TF_WORKSPACE=dev terraform apply";
        let ships = of_command(line, &[], Looks::Every, cwd, &|_| None);
        assert_eq!(
            ships[0].target,
            Some(Target::TerraformWorkspace { name: "dev".into() })
        );
    }
}
