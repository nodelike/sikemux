//! What would stop a task from starting, found before it runs, worded for
//! the person and the agent that asked.

use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

use super::detect::build_tool_of;
use super::java::JavaChoice;
use super::{read_text, tools};

const NODE_MANAGERS: [&str; 4] = ["npm", "pnpm", "yarn", "bun"];
const SPRING_PROPERTIES: [&str; 3] = [
    "src/main/resources/application.properties",
    "src/main/resources/application.yml",
    "src/main/resources/application.yaml",
];
const FREE_PORT_TRIES: u16 = 50;

pub struct Checked<'a> {
    pub command: &'a str,
    pub cwd: &'a Path,
    pub env: &'a [(String, String)],
}

impl Checked<'_> {
    fn env(&self, key: &str) -> Option<&str> {
        self.env
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }
}

/// Who listens on a port.
pub enum Listening {
    Free,
    /// An earlier run of this same task, which a restart frees.
    Ours,
    Held(String),
}

#[derive(Clone, Debug, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum CheckItem {
    BuildTool {
        ok: bool,
        name: String,
        program: String,
    },
    Java {
        ok: bool,
        required: u32,
        using: Option<u32>,
        current: Option<u32>,
    },
    Deps {
        ok: bool,
        install: String,
    },
    Port {
        ok: bool,
        port: u16,
        held_by: Option<String>,
        /// The next port nothing listens on, to move the app to.
        free_port: Option<u16>,
        /// The environment variable that moves the app to another port.
        env_var: Option<String>,
    },
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    #[serde(flatten)]
    pub item: CheckItem,
    pub title: String,
    pub detail: String,
}

impl CheckItem {
    pub fn ok(&self) -> bool {
        match self {
            Self::BuildTool { ok, .. }
            | Self::Java { ok, .. }
            | Self::Deps { ok, .. }
            | Self::Port { ok, .. } => *ok,
        }
    }

    fn worded(self) -> Check {
        let (title, detail) = match &self {
            Self::BuildTool {
                ok: true,
                name,
                program,
            } => (name.clone(), format!("{program} is installed")),
            Self::BuildTool { name, program, .. } => (
                format!("{name} isn't installed"),
                format!("This process runs {program}"),
            ),
            Self::Java {
                ok: true,
                required,
                using,
                current,
            } => (
                format!("Java {}", using.unwrap_or(*required)),
                if using == current {
                    format!("The project asks for Java {required}")
                } else {
                    format!(
                        "The project asks for Java {required}; your JAVA_HOME is {}",
                        current.map_or_else(|| "not set".to_owned(), |major| major.to_string())
                    )
                },
            ),
            Self::Java {
                required, current, ..
            } => (
                format!("Java {required} needed"),
                format!(
                    "The project asks for Java {required}; {} and nothing newer is installed",
                    current.map_or_else(
                        || "no Java is set".to_owned(),
                        |major| format!("your Java is {major}")
                    )
                ),
            ),
            Self::Deps { ok: true, .. } => (
                "Dependencies installed".to_owned(),
                "node_modules is there".to_owned(),
            ),
            Self::Deps { install, .. } => (
                "Dependencies aren't installed".to_owned(),
                format!("There is no node_modules yet; Install runs {install}"),
            ),
            Self::Port { ok: true, port, .. } => (
                format!("Port {port} is free"),
                "Where the app will listen".to_owned(),
            ),
            Self::Port {
                port,
                held_by,
                free_port,
                ..
            } => {
                let held_by = held_by.as_deref().unwrap_or("another program");
                (
                    format!("Port {port} is in use"),
                    match free_port {
                        Some(free) => format!("Held by {held_by}; Fix runs the app on {free}"),
                        None => format!("Held by {held_by}; the app can't start until it's free"),
                    },
                )
            }
        };
        Check {
            item: self,
            title,
            detail,
        }
    }
}

struct AppPort {
    port: u16,
    env_var: Option<&'static str>,
}

fn number(text: Option<&str>) -> Option<u16> {
    text?.parse().ok().filter(|port| *port > 0)
}

static SPRING_TASK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"spring-boot:run|bootRun").expect("pattern"));
static SPRING_PROPERTY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*server\.port\s*[=:]\s*([0-9]+)").expect("pattern"));
static SPRING_YAML: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^server:\s*\n(?:\s+.*\n)*?\s+port:\s*([0-9]+)").expect("pattern")
});
static NEXT_SCRIPT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?-u:\b)next\s+(dev|start)(?-u:\b)").expect("pattern"));
static PORT_FLAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:-p|--port)[\s=]([0-9]+)").expect("pattern"));
static LONG_PORT_FLAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"--port[\s=]([0-9]+)").expect("pattern"));
static UVICORN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"-m uvicorn(?-u:\b)").expect("pattern"));
static FLASK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"-m flask(?-u:\b).*(?-u:\b)run(?-u:\b)").expect("pattern"));
static DJANGO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"manage\.py runserver(?-u:\b)").expect("pattern"));
static DJANGO_ADDRESS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"runserver\s+(?:[\w.]+:)?([0-9]+)").expect("pattern"));

fn captured<'a>(pattern: &Regex, text: &'a str) -> Option<&'a str> {
    Some(pattern.captures(text)?.get(1)?.as_str())
}

/// Spring Boot listens on `server.port`, then `SERVER_PORT`, then 8080.
fn spring_port(task: &Checked) -> Option<AppPort> {
    if !SPRING_TASK.is_match(task.command) {
        return None;
    }
    let env_var = Some("SERVER_PORT");
    if let Some(port) = number(task.env("SERVER_PORT")) {
        return Some(AppPort { port, env_var });
    }
    for file in SPRING_PROPERTIES {
        let text = read_text(&task.cwd.join(file)).unwrap_or_default();
        let found = captured(&SPRING_PROPERTY, &text).or_else(|| captured(&SPRING_YAML, &text));
        if let Some(port) = number(found) {
            return Some(AppPort { port, env_var });
        }
    }
    Some(AppPort {
        port: 8080,
        env_var,
    })
}

/// Next.js serves on `-p`, then `PORT`, then 3000. Vite is left out: it moves
/// to the next free port on its own and prints where.
fn node_port(task: &Checked) -> Option<AppPort> {
    let mut words = task.command.split(' ');
    let (manager, verb, script) = (words.next()?, words.next()?, words.next()?);
    if !NODE_MANAGERS.contains(&manager) || verb != "run" || script.is_empty() {
        return None;
    }
    let manifest: serde_json::Value =
        serde_json::from_str(&read_text(&task.cwd.join("package.json"))?).ok()?;
    let body = manifest.get("scripts")?.get(script)?.as_str()?;
    if !NEXT_SCRIPT.is_match(body) {
        return None;
    }
    let port = number(captured(&PORT_FLAG, body))
        .or_else(|| number(task.env("PORT")))
        .unwrap_or(3000);
    Some(AppPort {
        port,
        env_var: Some("PORT"),
    })
}

/// uvicorn reads `--port` or `UVICORN_PORT`, Flask `--port` or
/// `FLASK_RUN_PORT`; Django's runserver takes the port as an argument only.
fn python_port(task: &Checked) -> Option<AppPort> {
    let flag = number(captured(&LONG_PORT_FLAG, task.command));
    let (variable, fallback) = if UVICORN.is_match(task.command) {
        ("UVICORN_PORT", 8000)
    } else if FLASK.is_match(task.command) {
        ("FLASK_RUN_PORT", 5000)
    } else if DJANGO.is_match(task.command) {
        let port = number(captured(&DJANGO_ADDRESS, task.command)).unwrap_or(8000);
        return Some(AppPort {
            port,
            env_var: None,
        });
    } else {
        return None;
    };
    Some(match flag {
        Some(port) => AppPort {
            port,
            env_var: None,
        },
        None => AppPort {
            port: number(task.env(variable)).unwrap_or(fallback),
            env_var: Some(variable),
        },
    })
}

fn dependencies(task: &Checked) -> Option<CheckItem> {
    let manager = task.command.split(' ').next()?;
    if !NODE_MANAGERS.contains(&manager) || !task.cwd.join("package.json").is_file() {
        return None;
    }
    Some(CheckItem::Deps {
        ok: task.cwd.join("node_modules").is_dir(),
        install: format!("{manager} install"),
    })
}

/// Every check for `task`, passing or not. `listening` says who holds a port.
pub fn check(
    task: &Checked,
    java: Option<&JavaChoice>,
    listening: impl Fn(u16) -> Listening,
) -> Vec<Check> {
    let mut items = Vec::new();
    if let Some((program, name)) = build_tool_of(task.command) {
        items.push(CheckItem::BuildTool {
            ok: tools::installed(program),
            name: name.to_owned(),
            program: program.to_owned(),
        });
    }
    if let Some(java) = java {
        items.push(CheckItem::Java {
            ok: java.runtime.is_some(),
            required: java.required,
            using: java.runtime.as_ref().map(|runtime| runtime.major),
            current: java.current.as_ref().map(|runtime| runtime.major),
        });
    }
    items.extend(dependencies(task));
    if let Some(app) = spring_port(task)
        .or_else(|| node_port(task))
        .or_else(|| python_port(task))
    {
        let held_by = match listening(app.port) {
            Listening::Free | Listening::Ours => None,
            Listening::Held(holder) => Some(holder),
        };
        let free_port = match (&held_by, app.env_var) {
            (Some(_), Some(_)) => (1..=FREE_PORT_TRIES)
                .filter_map(|step| app.port.checked_add(step))
                .find(|next| matches!(listening(*next), Listening::Free)),
            _ => None,
        };
        items.push(CheckItem::Port {
            ok: held_by.is_none(),
            port: app.port,
            held_by,
            free_port,
            env_var: app.env_var.map(str::to_owned),
        });
    }
    items.into_iter().map(CheckItem::worded).collect()
}

fn install_hint(program: &str) -> String {
    let formula = tools::formula(program).unwrap_or(program);
    if cfg!(target_os = "macos") {
        format!("Install it with: brew install {formula}")
    } else {
        format!("Install {program} with this host's package manager")
    }
}

/// One line per problem, for an agent that asked to start the task.
pub fn problems(checks: &[Check]) -> Option<String> {
    let lines: Vec<String> = checks
        .iter()
        .filter(|check| !check.item.ok())
        .map(|check| {
            let Check {
                item,
                title,
                detail,
            } = check;
            match item {
                CheckItem::BuildTool { program, .. } => {
                    format!("{title}. {detail}. {}", install_hint(program))
                }
                CheckItem::Java { required, .. } if cfg!(target_os = "macos") => {
                    format!("{title}. {detail}. Install it with: brew install openjdk@{required}")
                }
                CheckItem::Java { required, .. } => {
                    format!("{title}. {detail}. Install a Java {required} JDK on this host.")
                }
                CheckItem::Deps { install, .. } => {
                    format!("{title}. Run `{install}` in the process's folder first.")
                }
                CheckItem::Port { .. } => format!("{title}. {detail}."),
            }
        })
        .collect();
    (!lines.is_empty()).then(|| lines.join("\n"))
}
