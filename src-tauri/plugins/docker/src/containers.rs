//! Containers: listing them, reading one, and starting, stopping and removing them.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::engine::{query_value, segment, Engine};
use crate::error::{DockerError, DockerResult};

const COMPOSE_PROJECT: &str = "com.docker.compose.project";
const COMPOSE_SERVICE: &str = "com.docker.compose.service";
const COMPOSE_NUMBER: &str = "com.docker.compose.container-number";
const COMPOSE_DIR: &str = "com.docker.compose.project.working_dir";
const COMPOSE_FILES: &str = "com.docker.compose.project.config_files";

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Container {
    pub id: String,
    pub name: String,
    pub image: String,
    pub command: String,
    /// Seconds since 1970.
    pub created: i64,
    /// created, running, paused, restarting, removing, exited or dead.
    pub state: String,
    /// The engine's own words, such as "Up 2 hours (healthy)" or "Exited (1) 3 minutes ago".
    pub status: String,
    pub ports: Vec<Port>,
    pub compose: Option<Compose>,
}

#[derive(Debug, Serialize, PartialEq, Eq, PartialOrd, Ord, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Port {
    pub private: u16,
    pub public: Option<u16>,
    pub ip: Option<String>,
    pub protocol: String,
}

#[derive(Debug, Serialize, PartialEq, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Compose {
    pub project: String,
    pub service: String,
    pub number: Option<u32>,
    pub working_dir: Option<String>,
    pub config_files: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Summary {
    id: String,
    #[serde(default)]
    names: Vec<String>,
    #[serde(default)]
    image: String,
    #[serde(default)]
    command: String,
    #[serde(default)]
    created: i64,
    #[serde(default)]
    state: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    ports: Vec<SummaryPort>,
    #[serde(default)]
    labels: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct SummaryPort {
    #[serde(rename = "IP")]
    ip: Option<String>,
    private_port: u16,
    public_port: Option<u16>,
    #[serde(rename = "Type", default)]
    protocol: String,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ListParams {
    /// Stopped containers too, as `docker ps -a` shows them.
    pub all: Option<bool>,
    pub project: Option<String>,
    /// Words in the container's name or image.
    pub query: Option<String>,
    pub state: Option<String>,
}

pub fn short_id(id: &str) -> String {
    id.chars().take(12).collect()
}

fn compose_of(labels: &BTreeMap<String, String>) -> Option<Compose> {
    let project = labels.get(COMPOSE_PROJECT)?.clone();
    Some(Compose {
        project,
        service: labels.get(COMPOSE_SERVICE).cloned().unwrap_or_default(),
        number: labels
            .get(COMPOSE_NUMBER)
            .and_then(|number| number.parse().ok()),
        working_dir: labels.get(COMPOSE_DIR).cloned(),
        config_files: labels
            .get(COMPOSE_FILES)
            .map(|files| {
                files
                    .split(',')
                    .map(str::trim)
                    .filter(|file| !file.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// The engine lists a port once per address it is bound on, so a port bound
/// on both IPv4 and IPv6 is kept once, with the address that has one.
fn tidy(mut ports: Vec<Port>) -> Vec<Port> {
    ports.sort_by(|a, b| {
        (a.private, a.public, &a.protocol)
            .cmp(&(b.private, b.public, &b.protocol))
            .then_with(|| b.ip.cmp(&a.ip))
    });
    ports.dedup_by(|later, kept| {
        later.private == kept.private
            && later.public == kept.public
            && later.protocol == kept.protocol
    });
    ports
}

fn ports_of(ports: Vec<SummaryPort>) -> Vec<Port> {
    let ports = ports
        .into_iter()
        .map(|port| Port {
            private: port.private_port,
            public: port.public_port,
            ip: port.ip.filter(|ip| !ip.is_empty() && ip != "::"),
            protocol: if port.protocol.is_empty() {
                "tcp".to_owned()
            } else {
                port.protocol
            },
        })
        .collect();
    tidy(ports)
}

fn container_of(summary: Summary) -> Container {
    let name = summary
        .names
        .first()
        .map(|name| name.trim_start_matches('/').to_owned())
        .unwrap_or_else(|| short_id(&summary.id));
    Container {
        id: short_id(&summary.id),
        name,
        image: summary.image,
        command: summary.command,
        created: summary.created,
        state: summary.state,
        status: summary.status,
        ports: ports_of(summary.ports),
        compose: compose_of(&summary.labels),
    }
}

pub async fn list(engine: &Engine, params: ListParams) -> DockerResult<Vec<Container>> {
    let mut filters = BTreeMap::new();
    if let Some(project) = params
        .project
        .as_deref()
        .filter(|project| !project.is_empty())
    {
        filters.insert("label", vec![format!("{COMPOSE_PROJECT}={project}")]);
    }
    if let Some(state) = params.state.as_deref().filter(|state| !state.is_empty()) {
        filters.insert(
            "status",
            state
                .split(',')
                .map(|each| each.trim().to_owned())
                .collect(),
        );
    }
    let all = params.all.unwrap_or(true) || params.state.is_some();
    let mut path = format!("/containers/json?all={all}");
    if !filters.is_empty() {
        let encoded = serde_json::to_string(&filters)?;
        path.push_str(&format!("&filters={}", query_value(&encoded)));
    }
    let summaries: Vec<Summary> = engine.get(&path).await?;
    let words = params
        .query
        .as_deref()
        .map(str::to_lowercase)
        .filter(|words| !words.trim().is_empty());
    let mut containers: Vec<Container> = summaries
        .into_iter()
        .map(container_of)
        .filter(|container| {
            words.as_deref().is_none_or(|words| {
                container.name.to_lowercase().contains(words)
                    || container.image.to_lowercase().contains(words)
            })
        })
        .collect();
    containers.sort_by(|a, b| {
        let running = |container: &Container| container.state != "running";
        (running(a), &a.name).cmp(&(running(b), &b.name))
    });
    Ok(containers)
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Detail {
    pub id: String,
    pub name: String,
    pub image: String,
    pub image_id: String,
    pub created: String,
    pub command: Vec<String>,
    pub entrypoint: Vec<String>,
    pub working_dir: String,
    pub state: State,
    pub restart_count: u32,
    pub restart_policy: String,
    /// The names of its environment variables. Their values are never given.
    pub env: Vec<String>,
    pub mounts: Vec<Mount>,
    pub networks: Vec<Network>,
    pub ports: Vec<Port>,
    pub labels: BTreeMap<String, String>,
    pub compose: Option<Compose>,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct State {
    pub status: String,
    pub running: bool,
    pub exit_code: i64,
    pub error: Option<String>,
    pub oom_killed: bool,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub health: Option<Health>,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Health {
    pub status: String,
    pub failing_streak: u32,
    /// The output of the last few health checks, newest last.
    pub last_output: Vec<String>,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Mount {
    pub kind: String,
    pub source: String,
    pub destination: String,
    pub writable: bool,
    pub volume: Option<String>,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Network {
    pub name: String,
    pub ip: Option<String>,
    pub aliases: Vec<String>,
}

mod raw {
    use std::collections::BTreeMap;

    use serde::Deserialize;

    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    pub struct Inspect {
        pub id: String,
        #[serde(default)]
        pub name: String,
        #[serde(default)]
        pub image: String,
        #[serde(default)]
        pub created: String,
        #[serde(default)]
        pub restart_count: u32,
        pub config: Config,
        pub state: State,
        #[serde(default)]
        pub mounts: Vec<Mount>,
        pub host_config: Option<HostConfig>,
        pub network_settings: Option<NetworkSettings>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    pub struct Config {
        #[serde(default)]
        pub image: String,
        pub cmd: Option<Vec<String>>,
        pub entrypoint: Option<Vec<String>>,
        #[serde(default)]
        pub working_dir: String,
        pub env: Option<Vec<String>>,
        pub labels: Option<BTreeMap<String, String>>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    pub struct State {
        #[serde(default)]
        pub status: String,
        #[serde(default)]
        pub running: bool,
        #[serde(default)]
        pub exit_code: i64,
        #[serde(default)]
        pub error: String,
        #[serde(rename = "OOMKilled", default)]
        pub oom_killed: bool,
        #[serde(default)]
        pub started_at: String,
        #[serde(default)]
        pub finished_at: String,
        pub health: Option<Health>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    pub struct Health {
        #[serde(default)]
        pub status: String,
        #[serde(default)]
        pub failing_streak: u32,
        pub log: Option<Vec<HealthCheck>>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    pub struct HealthCheck {
        #[serde(default)]
        pub output: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    pub struct Mount {
        #[serde(rename = "Type", default)]
        pub kind: String,
        pub name: Option<String>,
        #[serde(default)]
        pub source: String,
        #[serde(default)]
        pub destination: String,
        #[serde(rename = "RW", default)]
        pub writable: bool,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    pub struct HostConfig {
        pub restart_policy: Option<RestartPolicy>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    pub struct RestartPolicy {
        #[serde(default)]
        pub name: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    pub struct NetworkSettings {
        pub networks: Option<BTreeMap<String, Endpoint>>,
        pub ports: Option<BTreeMap<String, Option<Vec<Binding>>>>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    pub struct Endpoint {
        #[serde(rename = "IPAddress", default)]
        pub ip_address: String,
        pub aliases: Option<Vec<String>>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    pub struct Binding {
        pub host_ip: Option<String>,
        pub host_port: Option<String>,
    }
}

const HEALTH_LINES: usize = 3;

fn some(text: String) -> Option<String> {
    let text = text.trim().to_owned();
    (!text.is_empty() && !text.starts_with("0001-01-01")).then_some(text)
}

/// `8080/tcp` with its bindings, as the engine writes a container's ports.
fn ports_of_inspect(ports: BTreeMap<String, Option<Vec<raw::Binding>>>) -> Vec<Port> {
    let mut found = Vec::new();
    for (key, bindings) in ports {
        let (private, protocol) = key.split_once('/').unwrap_or((key.as_str(), "tcp"));
        let Ok(private) = private.parse::<u16>() else {
            continue;
        };
        let bindings = bindings.unwrap_or_default();
        if bindings.is_empty() {
            found.push(Port {
                private,
                public: None,
                ip: None,
                protocol: protocol.to_owned(),
            });
        }
        for binding in bindings {
            found.push(Port {
                private,
                public: binding.host_port.and_then(|port| port.parse().ok()),
                ip: binding.host_ip.filter(|ip| !ip.is_empty() && ip != "::"),
                protocol: protocol.to_owned(),
            });
        }
    }
    tidy(found)
}

fn detail_of(inspect: raw::Inspect) -> Detail {
    let labels = inspect.config.labels.unwrap_or_default();
    let env = inspect
        .config
        .env
        .unwrap_or_default()
        .into_iter()
        .filter_map(|pair| {
            let name = pair.split_once('=').map_or(pair.as_str(), |(name, _)| name);
            (!name.is_empty()).then(|| name.to_owned())
        })
        .collect();
    let settings = inspect.network_settings;
    let (networks, ports) = match settings {
        Some(settings) => (
            settings
                .networks
                .unwrap_or_default()
                .into_iter()
                .map(|(name, endpoint)| Network {
                    name,
                    ip: some(endpoint.ip_address),
                    aliases: endpoint.aliases.unwrap_or_default(),
                })
                .collect(),
            ports_of_inspect(settings.ports.unwrap_or_default()),
        ),
        None => (Vec::new(), Vec::new()),
    };
    let state = inspect.state;
    Detail {
        id: short_id(&inspect.id),
        name: inspect.name.trim_start_matches('/').to_owned(),
        image: inspect.config.image,
        image_id: inspect.image,
        created: inspect.created,
        command: inspect.config.cmd.unwrap_or_default(),
        entrypoint: inspect.config.entrypoint.unwrap_or_default(),
        working_dir: inspect.config.working_dir,
        state: State {
            status: state.status,
            running: state.running,
            exit_code: state.exit_code,
            error: some(state.error),
            oom_killed: state.oom_killed,
            started_at: some(state.started_at),
            finished_at: some(state.finished_at),
            health: state.health.map(|health| {
                let log = health.log.unwrap_or_default();
                let skip = log.len().saturating_sub(HEALTH_LINES);
                Health {
                    status: health.status,
                    failing_streak: health.failing_streak,
                    last_output: log
                        .into_iter()
                        .skip(skip)
                        .map(|check| check.output.trim().to_owned())
                        .collect(),
                }
            }),
        },
        restart_count: inspect.restart_count,
        restart_policy: inspect
            .host_config
            .and_then(|config| config.restart_policy)
            .map(|policy| policy.name)
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "no".to_owned()),
        env,
        mounts: inspect
            .mounts
            .into_iter()
            .map(|mount| Mount {
                kind: mount.kind,
                source: mount.source,
                destination: mount.destination,
                writable: mount.writable,
                volume: mount.name.filter(|name| !name.is_empty()),
            })
            .collect(),
        networks,
        ports,
        compose: compose_of(&labels),
        labels,
    }
}

#[derive(Deserialize)]
pub struct Target {
    pub id: String,
}

pub async fn inspect(engine: &Engine, target: Target) -> DockerResult<Detail> {
    let id = segment(&target.id)?;
    let raw: raw::Inspect = engine.get(&format!("/containers/{id}/json")).await?;
    Ok(detail_of(raw))
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Start,
    Stop,
    Restart,
    Pause,
    Unpause,
    Kill,
}

impl Action {
    fn path(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Restart => "restart",
            Self::Pause => "pause",
            Self::Unpause => "unpause",
            Self::Kill => "kill",
        }
    }
}

#[derive(Deserialize)]
pub struct ActParams {
    pub id: String,
    pub action: Action,
}

/// Docker waits this long for a container to stop before killing it.
const STOP_GRACE_SECS: u32 = 10;

pub async fn act(engine: &Engine, params: ActParams) -> DockerResult<Detail> {
    let id = segment(&params.id)?;
    let wait = match params.action {
        Action::Stop | Action::Restart => format!("?t={STOP_GRACE_SECS}"),
        _ => String::new(),
    };
    engine
        .post(
            &format!("/containers/{id}/{}{wait}", params.action.path()),
            None,
        )
        .await?;
    inspect(engine, Target { id: id.to_owned() }).await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveParams {
    pub id: String,
    /// Stop it first if it is running.
    #[serde(default)]
    pub force: bool,
    /// Remove its anonymous volumes too.
    #[serde(default)]
    pub volumes: bool,
}

pub async fn remove(engine: &Engine, params: RemoveParams) -> DockerResult<()> {
    let id = segment(&params.id)?;
    engine
        .delete(&format!(
            "/containers/{id}?force={}&v={}",
            params.force, params.volumes
        ))
        .await
        .map_err(|error| match error {
            DockerError::Conflict(message) if !params.force => {
                DockerError::Conflict(format!("{message}. Stop it first, or remove it with force"))
            }
            other => other,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::tests::fake_engine;

    const LISTED: &str = r#"[
        {"Id":"aaaaaaaaaaaa1111","Names":["/shop-db-1"],"Image":"postgres:16","Command":"docker-entrypoint.sh postgres","Created":1700000000,"State":"exited","Status":"Exited (1) 3 minutes ago","Ports":[],
         "Labels":{"com.docker.compose.project":"shop","com.docker.compose.service":"db","com.docker.compose.container-number":"1","com.docker.compose.project.working_dir":"/Users/me/shop","com.docker.compose.project.config_files":"/Users/me/shop/compose.yaml"}},
        {"Id":"bbbbbbbbbbbb2222","Names":["/shop-web-1"],"Image":"shop-web","Command":"npm start","Created":1700000100,"State":"running","Status":"Up 2 hours","Ports":[
            {"IP":"0.0.0.0","PrivatePort":3000,"PublicPort":3000,"Type":"tcp"},{"IP":"::","PrivatePort":3000,"PublicPort":3000,"Type":"tcp"},{"PrivatePort":9229,"Type":"tcp"}],
         "Labels":{"com.docker.compose.project":"shop","com.docker.compose.service":"web"}},
        {"Id":"cccccccccccc3333","Names":["/redis"],"Image":"redis:7","Command":"redis-server","Created":1700000200,"State":"running","Status":"Up 5 minutes","Ports":[],"Labels":{}}
    ]"#;

    #[tokio::test]
    async fn running_containers_come_first_with_their_compose_project_and_ports_once() {
        let mut fake = fake_engine(vec![(200, "application/json", LISTED.to_owned())]);
        let containers = list(&fake.engine, ListParams::default()).await.unwrap();
        let names: Vec<&str> = containers
            .iter()
            .map(|container| container.name.as_str())
            .collect();
        assert_eq!(names, ["redis", "shop-web-1", "shop-db-1"]);
        let web = &containers[1];
        assert_eq!(web.id, "bbbbbbbbbbbb");
        assert_eq!(
            web.ports,
            [
                Port {
                    private: 3000,
                    public: Some(3000),
                    ip: Some("0.0.0.0".to_owned()),
                    protocol: "tcp".to_owned()
                },
                Port {
                    private: 9229,
                    public: None,
                    ip: None,
                    protocol: "tcp".to_owned()
                },
            ]
        );
        let db = containers[2].compose.as_ref().unwrap();
        assert_eq!(
            (db.project.as_str(), db.service.as_str(), db.number),
            ("shop", "db", Some(1))
        );
        assert_eq!(db.config_files, ["/Users/me/shop/compose.yaml"]);
        assert!(containers[0].compose.is_none());
        assert!(fake
            .seen
            .recv()
            .await
            .unwrap()
            .starts_with("GET /containers/json?all=true HTTP/1.1"));
    }

    #[tokio::test]
    async fn a_project_and_state_are_asked_of_the_engine_and_words_are_matched_here() {
        let mut fake = fake_engine(vec![(200, "application/json", LISTED.to_owned())]);
        let containers = list(
            &fake.engine,
            ListParams {
                project: Some("shop".to_owned()),
                state: Some("running".to_owned()),
                query: Some("WEB".to_owned()),
                all: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(containers.len(), 1);
        let request = fake.seen.recv().await.unwrap();
        let line = request.lines().next().unwrap();
        assert!(line.contains("filters=%7B%22label%22%3A%5B%22com.docker.compose.project%3Dshop%22%5D%2C%22status%22%3A%5B%22running%22%5D%7D"), "{line}");
    }

    const INSPECTED: &str = r#"{
        "Id":"aaaaaaaaaaaa1111ffff","Name":"/shop-db-1","Image":"sha256:1234","Created":"2026-10-18T10:00:00Z","RestartCount":2,
        "Config":{"Image":"postgres:16","Cmd":["postgres"],"Entrypoint":["docker-entrypoint.sh"],"WorkingDir":"","Env":["POSTGRES_PASSWORD=hunter2","PATH=/usr/bin","EMPTY="],"Labels":{"com.docker.compose.project":"shop","com.docker.compose.service":"db"}},
        "State":{"Status":"exited","Running":false,"ExitCode":1,"Error":"","OOMKilled":false,"StartedAt":"2026-10-18T10:00:01Z","FinishedAt":"2026-10-18T10:00:05Z",
                 "Health":{"Status":"unhealthy","FailingStreak":3,"Log":[{"Output":"a"},{"Output":"b"},{"Output":"c"},{"Output":"pg_isready: no response\n"}]}},
        "Mounts":[{"Type":"volume","Name":"shop_data","Source":"/var/lib/docker/volumes/shop_data/_data","Destination":"/var/lib/postgresql/data","RW":true}],
        "HostConfig":{"RestartPolicy":{"Name":"unless-stopped"}},
        "NetworkSettings":{"Networks":{"shop_default":{"IPAddress":"","Aliases":["db"]}},"Ports":{"5432/tcp":[{"HostIp":"127.0.0.1","HostPort":"5432"}],"8080/tcp":null}}
    }"#;

    #[tokio::test]
    async fn a_container_reads_with_its_env_names_but_never_their_values() {
        let fake = fake_engine(vec![(200, "application/json", INSPECTED.to_owned())]);
        let detail = inspect(
            &fake.engine,
            Target {
                id: "shop-db-1".to_owned(),
            },
        )
        .await
        .unwrap();
        assert_eq!(detail.env, ["POSTGRES_PASSWORD", "PATH", "EMPTY"]);
        assert!(!serde_json::to_string(&detail).unwrap().contains("hunter2"));
        assert_eq!((detail.state.exit_code, detail.restart_count), (1, 2));
        assert_eq!(detail.restart_policy, "unless-stopped");
        let health = detail.state.health.unwrap();
        assert_eq!(health.last_output, ["b", "c", "pg_isready: no response"]);
        assert_eq!(detail.mounts[0].volume.as_deref(), Some("shop_data"));
        assert_eq!(detail.networks[0].ip, None);
        assert_eq!(
            detail.ports,
            [
                Port {
                    private: 5432,
                    public: Some(5432),
                    ip: Some("127.0.0.1".to_owned()),
                    protocol: "tcp".to_owned()
                },
                Port {
                    private: 8080,
                    public: None,
                    ip: None,
                    protocol: "tcp".to_owned()
                },
            ]
        );
    }

    #[tokio::test]
    async fn stopping_waits_out_the_grace_period_and_answers_with_the_container_after() {
        let mut fake = fake_engine(vec![
            (204, "text/plain", String::new()),
            (200, "application/json", INSPECTED.to_owned()),
        ]);
        let detail = act(
            &fake.engine,
            ActParams {
                id: "shop-db-1".to_owned(),
                action: Action::Stop,
            },
        )
        .await
        .unwrap();
        assert_eq!(detail.name, "shop-db-1");
        assert!(fake
            .seen
            .recv()
            .await
            .unwrap()
            .starts_with("POST /containers/shop-db-1/stop?t=10 HTTP/1.1"));
    }

    #[tokio::test]
    async fn removing_a_running_container_without_force_says_how_to() {
        let fake = fake_engine(vec![(
            409,
            "application/json",
            r#"{"message":"cannot remove container: container is running"}"#.to_owned(),
        )]);
        let error = remove(
            &fake.engine,
            RemoveParams {
                id: "redis".to_owned(),
                force: false,
                volumes: false,
            },
        )
        .await
        .unwrap_err();
        assert!(
            error
                .to_string()
                .ends_with("Stop it first, or remove it with force"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn a_name_that_would_leave_its_path_is_turned_away_before_any_request() {
        let fake = fake_engine(vec![]);
        let error = inspect(
            &fake.engine,
            Target {
                id: "../info".to_owned(),
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(error, DockerError::BadArg(_)));
    }
}
