//! The docker-compose services a project's folders need running, and starting
//! or stopping them with `docker compose`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::connection::blocking;
use super::{Core, CoreResult};

const COMPOSE_FILES: [&str; 4] = [
    "compose.yaml",
    "compose.yml",
    "docker-compose.yaml",
    "docker-compose.yml",
];
const READ_TIMEOUT: Duration = Duration::from_secs(10);
const START_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Service {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    pub ports: Vec<String>,
    /// As docker reports it, such as `running` or `exited`; none when it was never created.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ComposeFolder {
    /// Relative to the project, `.` for its root.
    pub folder: String,
    pub file: String,
    pub services: Vec<Service>,
    /// Whether docker answered; when not, states are unknown.
    pub docker: bool,
}

pub(crate) fn compose_file(dir: &Path) -> Option<PathBuf> {
    COMPOSE_FILES
        .iter()
        .map(|name| dir.join(name))
        .find(|path| path.is_file())
}

/// The services a compose file declares, read by indentation rather than a full YAML parser.
fn parse(text: &str) -> Vec<Service> {
    let mut services: Vec<Service> = Vec::new();
    let mut inside = false;
    let mut service_indent = None;
    let mut in_ports = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        if indent == 0 {
            inside = trimmed == "services:";
            service_indent = None;
            continue;
        }
        if !inside {
            continue;
        }
        let at = *service_indent.get_or_insert(indent);
        if indent == at {
            if let Some(name) = trimmed.strip_suffix(':') {
                services.push(Service {
                    name: name.trim_matches(['"', '\'']).to_owned(),
                    image: None,
                    ports: Vec::new(),
                    state: None,
                });
            }
            in_ports = false;
            continue;
        }
        let Some(service) = services.last_mut() else {
            continue;
        };
        if let Some(image) = trimmed.strip_prefix("image:") {
            service.image = Some(image.trim().trim_matches(['"', '\'']).to_owned());
            in_ports = false;
        } else if let Some(listed) = trimmed.strip_prefix("ports:") {
            let listed = listed.trim();
            in_ports = listed.is_empty();
            service.ports.extend(
                listed
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .split(',')
                    .map(|port| port.trim().trim_matches(['"', '\'']).to_owned())
                    .filter(|port| !port.is_empty()),
            );
        } else if in_ports && trimmed.starts_with('-') {
            let port = trimmed
                .trim_start_matches('-')
                .trim()
                .trim_matches(['"', '\'']);
            service.ports.push(port.to_owned());
        } else if !trimmed.starts_with('-') {
            in_ports = false;
        }
    }
    services
}

fn compose(dir: &Path, args: &[&str], timeout: Duration) -> Result<String, String> {
    let mut child = sikemux_process::user_environment::command("docker")
        .arg("compose")
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("docker could not start: {error}"))?;
    let started = Instant::now();
    loop {
        if child
            .try_wait()
            .map_err(|error| error.to_string())?
            .is_some()
        {
            break;
        }
        if started.elapsed() > timeout {
            let _ = child.kill();
            return Err(format!("docker compose {} took too long", args.join(" ")));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let output = child
        .wait_with_output()
        .map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        let said = String::from_utf8_lossy(&output.stderr);
        Err(said
            .trim()
            .lines()
            .last()
            .unwrap_or("docker compose failed")
            .to_owned())
    }
}

/// Each service's state, by name, from `docker compose ps`, which writes a JSON array or one object per line.
fn states(dir: &Path) -> Option<HashMap<String, String>> {
    let text = compose(dir, &["ps", "--all", "--format", "json"], READ_TIMEOUT).ok()?;
    let rows: Vec<Value> = match serde_json::from_str::<Value>(text.trim()) {
        Ok(Value::Array(rows)) => rows,
        Ok(row) => vec![row],
        Err(_) => text
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect(),
    };
    Some(
        rows.iter()
            .filter_map(|row| {
                let name = row.get("Service")?.as_str()?.to_owned();
                let state = row.get("State")?.as_str()?.to_owned();
                Some((name, state))
            })
            .collect(),
    )
}

fn folder_dir(project: &Path, folder: &str) -> PathBuf {
    if folder == "." {
        project.to_path_buf()
    } else {
        project.join(folder)
    }
}

/// The compose files at the project root and in the folders just inside it, with their services.
pub(crate) fn list(project: &Path) -> Vec<ComposeFolder> {
    std::iter::once(".".to_owned())
        .chain(crate::tasks::detect::inner_folders(project))
        .filter_map(|folder| {
            let dir = folder_dir(project, &folder);
            let file = compose_file(&dir)?;
            let mut services = parse(&crate::tasks::read_text(&file)?);
            let states = states(&dir);
            for service in &mut services {
                service.state = states
                    .as_ref()
                    .and_then(|states| states.get(&service.name).cloned());
            }
            Some(ComposeFolder {
                folder,
                file: file.file_name()?.to_string_lossy().into_owned(),
                services,
                docker: states.is_some(),
            })
        })
        .collect()
}

/// The folder whose compose file runs services for `folder`: its own, else the project root's.
pub(crate) fn compose_dir_for(project: &Path, folder: &str) -> Option<PathBuf> {
    let own = folder_dir(project, folder);
    if compose_file(&own).is_some() {
        return Some(own);
    }
    compose_file(project).map(|_| project.to_path_buf())
}

/// Starts `services` and waits until docker says they are up.
pub(crate) fn up(dir: &Path, services: &[String]) -> Result<(), String> {
    let mut args = vec!["up", "--detach", "--wait"];
    args.extend(services.iter().map(String::as_str));
    compose(dir, &args, START_TIMEOUT).map(|_| ())
}

#[derive(Deserialize)]
pub(crate) struct ActionParams {
    project: String,
    folder: String,
    service: String,
    action: String,
}

pub(crate) async fn action(params: ActionParams) -> CoreResult<()> {
    blocking(move || {
        let dir = folder_dir(Path::new(&params.project), &params.folder);
        match params.action.as_str() {
            "start" => up(&dir, &[params.service]),
            "stop" => compose(&dir, &["stop", &params.service], START_TIMEOUT).map(|_| ()),
            other => Err(format!("{other} is not start or stop")),
        }
        .map_err(Into::into)
    })
    .await
}

/// Starts what the folder of `cwd` needs before one of its tasks runs.
pub(crate) async fn before_task(core: &Core, cwd: &Path) -> CoreResult<()> {
    let Some((dir, services)) = core.workspaces.services_for(cwd) else {
        return Ok(());
    };
    blocking(move || {
        up(&dir, &services)
            .map_err(|error| format!("{} could not start: {error}", services.join(", ")).into())
    })
    .await
}
