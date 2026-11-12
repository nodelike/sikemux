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
    /// A database the database connector marks as production, by its connection's name.
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
    /// environments its jobs name, whether it publishes, whether its file could be read as
    /// GitHub will run it, and the inputs it was given.
    Workflow {
        file: String,
        environments: Vec<String>,
        publishes: bool,
        deploys: bool,
        read: bool,
        inputs: Vec<String>,
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
    /// The site or connector it writes to, as the person reads it.
    pub fn writes_to(&self) -> Option<&str> {
        match &self.target {
            Some(Target::Site { origin }) => Some(origin),
            Some(Target::Connector { name }) => Some(name),
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
    let program = base_name(program);
    if let Some((program, args)) = wrapped(program, args) {
        return target_of(program, args, env, cwd);
    }
    let named = |flags: &[&str]| flag_value(args, flags);
    match program {
        "vercel" => {
            let target = named(&["--target"]);
            let preview = target
                .as_deref()
                .is_some_and(|target| target != "production");
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
        "curl" | "wget" => (web_addresses(args).into_iter())
            .find(|address| !address.host_str().is_some_and(local_host))
            .map(|address| Target::Site {
                origin: address.origin().ascii_serialization(),
            }),
        "gh" if args.windows(2).any(|pair| pair == ["workflow", "run"]) => {
            Some(workflow(args, env, cwd))
        }
        "gh" if args.first().is_some_and(|word| word == "api") => {
            Some(stage("GitHub's API", "writes"))
        }
        "gh" => Some(package(GITHUB_RELEASES)),
        "npm" | "pnpm" | "yarn" if args.iter().any(|arg| arg == "publish") => Some(package("npm")),
        "changeset" => Some(package("npm")),
        "cargo" if args.iter().any(|arg| arg == "publish") => Some(package("crates.io")),
        "docker" => {
            let images: Vec<&String> = match args.first().map(String::as_str) {
                Some("push") => args[1..]
                    .iter()
                    .filter(|arg| !arg.starts_with('-'))
                    .take(1)
                    .collect(),
                _ => (args.windows(2))
                    .filter(|pair| matches!(pair[0].as_str(), "-t" | "--tag"))
                    .map(|pair| &pair[1])
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
                Some(at) => args[at + 1..]
                    .iter()
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
        _ => None,
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
    let rest = &args[skip..];
    let at = rest.iter().position(|arg| !arg.starts_with('-'))?;
    let (program, args) = rest[at..].split_first()?;
    let program = program.rsplit('/').next().unwrap_or(program);
    let program = program
        .split('@')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(program);
    Some((base_name(program), args))
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
        return value["projects"]["default"].as_str().map(str::to_owned);
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
        file: file.clone(),
        environments: Vec::new(),
        publishes: false,
        deploys: false,
        read: false,
        inputs: inputs.clone(),
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
        publishes: read.publishes,
        deploys: read.deploys,
        read: true,
        inputs,
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
    "twine",
    "poetry",
    "gem",
    "cdk",
    "heroku",
    "uv",
    "sst",
    "sam",
    "supabase",
    "prisma",
    "dokku",
    "psql",
    "mysql",
    "npx",
    "pnpx",
    "bunx",
    "changeset",
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
    rule(
        &["gh"],
        &["release", "create"],
        Needs::Nothing,
        ShipKind::Publish,
    ),
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
    rule(
        &["gh"],
        &["release", "edit"],
        Needs::Nothing,
        ShipKind::Publish,
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
    let program = base_name(program);
    if dry_run(args) {
        return None;
    }
    if program == "git" {
        let subcommand = git_args(args).first()?;
        return match subcommand.as_str() {
            "push" if git_args(args).iter().any(|arg| arg == "-n") => None,
            "push" => Some(ShipKind::Push),
            sub if !GIT_COMMANDS.contains(&sub) => {
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
        return web_write(program, args).then_some(ShipKind::Publish);
    }
    if let Some((program, args)) = wrapped(program, args) {
        return of_program(program, args, cwd, alias);
    }
    let first = args
        .iter()
        .find(|arg| !arg.starts_with('-'))
        .map(String::as_str);
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
    let mut words = args[at + 1..].iter();
    while let Some(word) = words.next() {
        if word.starts_with("--") && !word.contains('=') && !switches.contains(&word.as_str()) {
            words.next();
        } else if !word.starts_with('-') {
            paths.push(word);
        }
    }
    paths.get(1).copied()
}

/// Whether `gh api` with `args` writes: a GraphQL query that holds a mutation, or a REST call
/// with a method other than GET, or with fields and no method, which gh sends as a POST.
fn api_writes(args: &[String], cwd: &Path) -> bool {
    let at = args
        .iter()
        .position(|arg| arg == "api")
        .map_or(0, |at| at + 1);
    let endpoint = args[at..].iter().find(|arg| !arg.starts_with('-'));
    let method = flag_value(args, &["-X", "--method"]);
    if endpoint.is_some_and(|endpoint| endpoint == "graphql") {
        let fields = (args.windows(2))
            .filter(|pair| matches!(pair[0].as_str(), "-f" | "-F" | "--field" | "--raw-field"))
            .map(|pair| pair[1].as_str())
            .chain(args.iter().filter_map(|arg| {
                ["-f", "-F", "--field=", "--raw-field="]
                    .iter()
                    .find_map(|flag| arg.strip_prefix(flag).filter(|rest| rest.contains('=')))
            }));
        let mut query = None;
        for field in fields {
            if let Some(text) = field.strip_prefix("query=") {
                query = Some(text);
            }
        }
        let mutates = |text: &str| {
            text.split(|c: char| !c.is_ascii_alphanumeric())
                .any(|word| word == "mutation")
        };
        return match query {
            Some(file) if file.starts_with('@') => {
                std::fs::read_to_string(cwd.join(&file[1..])).map_or(true, |text| mutates(&text))
            }
            Some(text) => mutates(text),
            None => has_flag(args, &["--input"]),
        };
    }
    match method {
        Some(verb) => !verb.eq_ignore_ascii_case("GET"),
        None => has_flag(args, &["-f", "-F", "--field", "--raw-field", "--input"]),
    }
}

/// Vercel's commands that read, or work on this computer only.
const VERCEL_READS: [&str; 20] = [
    "ls", "list", "logs", "inspect", "whoami", "help", "pull", "env", "domains", "dns", "certs",
    "project", "projects", "teams", "login", "logout", "switch", "link", "dev", "build",
];

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

/// Whether a command says it only tries: `--dry-run` in any form but kubectl's `none`, aws
/// s3's `--dryrun`, or a request for its help, given as a flag of its own: after a word that
/// isn't an option, an option that carries its own value, or a switch known to take none.
/// After any other option it is that option's value, and the action is real.
fn dry_run(args: &[String]) -> bool {
    args.iter().enumerate().any(|(at, arg)| {
        let flag = arg == "--dry-run"
            || arg == "--dryrun"
            || arg == "--help"
            || (arg.strip_prefix("--dry-run=")).is_some_and(|value| value != "none");
        let value_of_another =
            (at.checked_sub(1).and_then(|before| args.get(before))).is_some_and(|before| {
                before.starts_with('-')
                    && !before.contains('=')
                    && !SWITCHES.contains(&before.as_str())
            });
        flag && !value_of_another
    })
}

/// Whether a database client is given only statements that read: each passed with `-c`,
/// `--command`, `-e` or `--execute` starts with SELECT, SHOW, EXPLAIN or DESCRIBE, or is one of
/// psql's listing commands.
fn only_queries(args: &[String]) -> bool {
    let given: Vec<&String> = (args.windows(2))
        .filter(|pair| matches!(pair[0].as_str(), "-c" | "--command" | "-e" | "--execute"))
        .map(|pair| &pair[1])
        .collect();
    let reads = |statement: &str| {
        let first = statement.split_whitespace().next().unwrap_or_default();
        ["select", "show", "explain", "describe", "desc"]
            .iter()
            .any(|read| first.eq_ignore_ascii_case(read))
            || (first.starts_with("\\d") || first == "\\l")
    };
    !given.is_empty()
        && given.iter().all(|text| {
            (text.split(';'))
                .map(|statement| statement.trim().trim_matches(['\'', '"']).trim())
                .filter(|statement| !statement.is_empty())
                .all(reads)
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

fn matches(rule: &Rule, args: &[String], cwd: &Path) -> bool {
    let follows = rule.words.is_empty()
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
/// the line runs under, beneath what it sets itself. A `cd` earlier on the line
/// leaves where a command lands open.
pub fn of_command(
    line: &str,
    extra: &[String],
    looks: Looks,
    cwd: &Path,
    env: &dyn Fn(&str) -> Option<String>,
) -> Vec<Ship> {
    let lines = segments(line);
    let mut here = Some(cwd.to_owned());
    let mut selected: Vec<String> = Vec::new();
    lines
        .iter()
        .flat_map(|all| {
            if matches!(all.first().map(String::as_str), Some("cd" | "pushd")) {
                here = match (here.take(), all.get(1)) {
                    (Some(at), Some(to))
                        if all.len() == 2 && !to.contains(['$', '`']) && to != "-" =>
                    {
                        let to = match to.strip_prefix('~') {
                            Some(rest) => env("HOME").map(|home| format!("{home}{rest}")),
                            None => Some(to.clone()),
                        };
                        to.map(|to| at.join(to))
                    }
                    _ => None,
                };
                return Vec::new();
            }
            if selects(all) {
                selected.push(all.join(" "));
                return Vec::new();
            }
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
                false => of_program(program, args, here.as_deref().unwrap_or(cwd), &|_| None),
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
            let at = here.clone().filter(|_| selected.is_empty());
            let target = match (at, matches!(base_name(program), "curl" | "wget")) {
                (Some(at), _) => target_of(program, args, &set, &at),
                (None, true) => target_of(program, args, &set, cwd),
                (None, false) => None,
            };
            if !leaves_machine(program, args, target.as_ref()) {
                return Vec::new();
            }
            let what = std::iter::once(words.join(" "));
            let configured = git_configured(program, &set);
            let what: Vec<String> = (configured.into_iter())
                .chain(selected.iter().cloned())
                .chain(what)
                .collect();
            vec![Ship {
                kind,
                what: what.join(" && "),
                target,
            }]
        })
        .collect()
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
pub(crate) fn of_invocation(
    program: &str,
    args: &[String],
    env: &dyn Fn(&str) -> Option<String>,
    cwd: &Path,
) -> Option<Ship> {
    let kind = of_program(program, args, cwd, &|_| None)?;
    let target = target_of(program, args, env, cwd);
    leaves_machine(program, args, target.as_ref()).then(|| Ship {
        kind,
        target,
        what: (git_configured(program, env).into_iter())
            .chain(std::iter::once(program.to_owned()))
            .chain(args.iter().cloned())
            .collect::<Vec<_>>()
            .join(" "),
    })
}

/// Git settings given through the environment, which the push judge can't read, as the words
/// that put them in front of the command.
fn git_configured(program: &str, env: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    let git = base_name(program) == "git";
    let set = [
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_PARAMETERS",
        "GIT_CONFIG",
        "GIT_DIR",
    ]
    .into_iter()
    .find(|name| env(name).is_some());
    set.filter(|_| git).map(|name| format!("{name}=…"))
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

/// When a Sikemux tool's call leaves this machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolShip {
    /// Every call does.
    Always(ShipKind),
    /// A call does when the database it names is marked as production and
    /// agents may change its data there.
    ProductionDatabase,
}

/// When the Sikemux tool `name` leaves this machine. A push through
/// git_commit is the core's own, checked where it pushes.
pub fn of_tool(name: &str) -> Option<ToolShip> {
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
        | "bitbucket_issue_state"
        | "play_upload" => ToolShip::Always(ShipKind::Publish),
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

/// The registry a GitHub release goes to.
pub const GITHUB_RELEASES: &str = "GitHub releases";
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
