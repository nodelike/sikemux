//! Whether an engine is running, which one, and how much it holds.

use serde::{Deserialize, Serialize};

use crate::engine::Engine;
use crate::error::DockerError;

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub running: bool,
    /// Why no engine answered, in words for the person.
    pub message: Option<String>,
    pub socket: Option<String>,
    /// `DOCKER_HOST`, the context's name, or the folder the socket was found in.
    pub source: Option<String>,
    pub engine: Option<EngineInfo>,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EngineInfo {
    pub name: String,
    pub version: String,
    pub api_version: String,
    pub os: String,
    pub arch: String,
    pub cpus: u32,
    pub memory: u64,
    pub containers: u32,
    pub running: u32,
    pub paused: u32,
    pub stopped: u32,
    pub images: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Version {
    version: String,
    api_version: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Info {
    #[serde(default)]
    name: String,
    #[serde(default)]
    operating_system: String,
    #[serde(default)]
    architecture: String,
    #[serde(rename = "NCPU", default)]
    ncpu: u32,
    #[serde(default)]
    mem_total: u64,
    #[serde(default)]
    containers: u32,
    #[serde(default)]
    containers_running: u32,
    #[serde(default)]
    containers_paused: u32,
    #[serde(default)]
    containers_stopped: u32,
    #[serde(default)]
    images: u32,
}

pub async fn status() -> Status {
    match Engine::find() {
        Ok(engine) => status_of(&engine).await,
        Err(error) => not_running(&error, None),
    }
}

pub async fn status_of(engine: &Engine) -> Status {
    let endpoint = engine.endpoint();
    let read = async {
        let version: Version = engine.get("/version").await?;
        let info: Info = engine.get("/info").await?;
        Ok::<_, DockerError>(EngineInfo {
            name: info.name,
            version: version.version,
            api_version: version.api_version,
            os: info.operating_system,
            arch: info.architecture,
            cpus: info.ncpu,
            memory: info.mem_total,
            containers: info.containers,
            running: info.containers_running,
            paused: info.containers_paused,
            stopped: info.containers_stopped,
            images: info.images,
        })
    };
    match read.await {
        Ok(info) => Status {
            running: true,
            message: None,
            socket: Some(endpoint.socket.display().to_string()),
            source: Some(endpoint.source.clone()),
            engine: Some(info),
        },
        Err(error) => not_running(&error, Some(engine)),
    }
}

fn not_running(error: &DockerError, engine: Option<&Engine>) -> Status {
    let message = match error {
        DockerError::NotRunning(message) | DockerError::Unsupported(message) => message.clone(),
        other => other.to_string(),
    };
    Status {
        running: false,
        message: Some(message),
        socket: engine.map(|engine| engine.endpoint().socket.display().to_string()),
        source: engine.map(|engine| engine.endpoint().source.clone()),
        engine: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::tests::fake_engine;

    #[tokio::test]
    async fn a_running_engine_reports_its_version_and_counts() {
        let fake = fake_engine(vec![
            (200, "application/json", r#"{"Version":"29.2.1","ApiVersion":"1.53"}"#.to_owned()),
            (
                200,
                "application/json",
                r#"{"Name":"colima","OperatingSystem":"Ubuntu 24.04","Architecture":"aarch64","NCPU":4,"MemTotal":8000000000,"Containers":5,"ContainersRunning":3,"ContainersPaused":0,"ContainersStopped":2,"Images":9}"#.to_owned(),
            ),
        ]);
        let status = status_of(&fake.engine).await;
        assert!(status.running);
        let engine = status.engine.unwrap();
        assert_eq!(
            (engine.name.as_str(), engine.version.as_str()),
            ("colima", "29.2.1")
        );
        assert_eq!((engine.running, engine.stopped, engine.images), (3, 2, 9));
    }

    #[tokio::test]
    async fn an_engine_that_stopped_answering_reads_as_not_running() {
        let silent = crate::engine::Engine::new(crate::engine::Endpoint {
            socket: crate::engine::tests::scratch("gone").join("docker.sock"),
            source: "colima".to_owned(),
        });
        let status = status_of(&silent).await;
        assert!(!status.running);
        assert_eq!(status.source.as_deref(), Some("colima"));
        assert!(status.message.unwrap().contains("Docker isn't running"));
    }
}
