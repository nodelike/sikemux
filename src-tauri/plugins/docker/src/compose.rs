//! Compose stacks, put together from the labels Compose leaves on each container,
//! and starting, stopping or taking down a whole stack at once.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::containers::{self, ActParams, Action, Container, ListParams, RemoveParams};
use crate::engine::Engine;
use crate::error::{DockerError, DockerResult};

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Stack {
    pub project: String,
    /// The folder `docker compose up` ran in, which holds its files.
    pub working_dir: Option<String>,
    pub config_files: Vec<String>,
    pub services: Vec<Service>,
    pub running: usize,
    pub total: usize,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Service {
    pub name: String,
    pub containers: Vec<Container>,
}

pub fn stacks_of(containers: Vec<Container>) -> Vec<Stack> {
    let mut stacks: BTreeMap<String, Stack> = BTreeMap::new();
    for container in containers {
        let Some(compose) = container.compose.clone() else {
            continue;
        };
        let stack = stacks
            .entry(compose.project.clone())
            .or_insert_with(|| Stack {
                project: compose.project.clone(),
                working_dir: None,
                config_files: Vec::new(),
                services: Vec::new(),
                running: 0,
                total: 0,
            });
        if stack.working_dir.is_none() {
            stack.working_dir = compose.working_dir.clone();
        }
        if stack.config_files.is_empty() {
            stack.config_files = compose.config_files.clone();
        }
        stack.total += 1;
        if container.state == "running" {
            stack.running += 1;
        }
        match stack
            .services
            .iter_mut()
            .find(|service| service.name == compose.service)
        {
            Some(service) => service.containers.push(container),
            None => stack.services.push(Service {
                name: compose.service.clone(),
                containers: vec![container],
            }),
        }
    }
    let mut stacks: Vec<Stack> = stacks.into_values().collect();
    for stack in &mut stacks {
        stack.services.sort_by(|a, b| a.name.cmp(&b.name));
    }
    stacks.sort_by(|a, b| (a.running == 0, &a.project).cmp(&(b.running == 0, &b.project)));
    stacks
}

pub async fn list(engine: &Engine) -> DockerResult<Vec<Stack>> {
    let containers = containers::list(engine, ListParams::default()).await?;
    Ok(stacks_of(containers))
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum StackAction {
    Start,
    Stop,
    Restart,
    /// Stops and removes the stack's containers, keeping its volumes and images.
    Down,
}

#[derive(Deserialize)]
pub struct StackParams {
    pub project: String,
    pub action: StackAction,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StackResult {
    pub project: String,
    /// The containers acted on.
    pub done: Vec<String>,
    /// The containers that refused, with the engine's reason.
    pub failed: Vec<Failure>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Failure {
    pub name: String,
    pub message: String,
}

/// Acts on every container of the stack. One that refuses doesn't stop the rest.
pub async fn act(engine: &Engine, params: StackParams) -> DockerResult<StackResult> {
    let project = params.project.trim();
    if project.is_empty() {
        return Err(DockerError::BadArg(
            "a Compose project's name is needed".to_owned(),
        ));
    }
    let containers = containers::list(
        engine,
        ListParams {
            project: Some(project.to_owned()),
            ..ListParams::default()
        },
    )
    .await?;
    if containers.is_empty() {
        return Err(DockerError::NotFound(format!(
            "no containers belong to the Compose project {project}"
        )));
    }
    let mut result = StackResult {
        project: project.to_owned(),
        done: Vec::new(),
        failed: Vec::new(),
    };
    for container in containers {
        let running = container.state == "running" || container.state == "restarting";
        let outcome = match params.action {
            StackAction::Start if running => continue,
            StackAction::Stop if !running => continue,
            StackAction::Start => run(engine, &container, Action::Start).await,
            StackAction::Stop => run(engine, &container, Action::Stop).await,
            StackAction::Restart => run(engine, &container, Action::Restart).await,
            StackAction::Down => {
                containers::remove(
                    engine,
                    RemoveParams {
                        id: container.id.clone(),
                        force: true,
                        volumes: false,
                    },
                )
                .await
            }
        };
        match outcome {
            Ok(()) => result.done.push(container.name),
            Err(error) => result.failed.push(Failure {
                name: container.name,
                message: error.to_string(),
            }),
        }
    }
    Ok(result)
}

async fn run(engine: &Engine, container: &Container, action: Action) -> DockerResult<()> {
    containers::act(
        engine,
        ActParams {
            id: container.id.clone(),
            action,
        },
    )
    .await
    .map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::containers::Compose;
    use crate::engine::tests::fake_engine;

    fn container(name: &str, project: Option<&str>, service: &str, state: &str) -> Container {
        Container {
            id: name.to_owned(),
            name: name.to_owned(),
            image: "img".to_owned(),
            command: String::new(),
            created: 0,
            state: state.to_owned(),
            status: String::new(),
            ports: Vec::new(),
            compose: project.map(|project| Compose {
                project: project.to_owned(),
                service: service.to_owned(),
                number: Some(1),
                working_dir: Some(format!("/Users/me/{project}")),
                config_files: vec![format!("/Users/me/{project}/compose.yaml")],
            }),
        }
    }

    #[test]
    fn containers_group_into_stacks_and_services_with_running_stacks_first() {
        let stacks = stacks_of(vec![
            container("blog-web-1", Some("blog"), "web", "exited"),
            container("shop-web-1", Some("shop"), "web", "running"),
            container("shop-web-2", Some("shop"), "web", "running"),
            container("shop-db-1", Some("shop"), "db", "exited"),
            container("redis", None, "", "running"),
        ]);
        let projects: Vec<&str> = stacks.iter().map(|stack| stack.project.as_str()).collect();
        assert_eq!(projects, ["shop", "blog"]);
        let shop = &stacks[0];
        assert_eq!((shop.running, shop.total), (2, 3));
        let services: Vec<(&str, usize)> = shop
            .services
            .iter()
            .map(|service| (service.name.as_str(), service.containers.len()))
            .collect();
        assert_eq!(services, [("db", 1), ("web", 2)]);
        assert_eq!(shop.working_dir.as_deref(), Some("/Users/me/shop"));
    }

    const SHOP: &str = r#"[
        {"Id":"aaaaaaaaaaaa","Names":["/shop-db-1"],"Image":"postgres","State":"exited","Status":"","Labels":{"com.docker.compose.project":"shop","com.docker.compose.service":"db"}},
        {"Id":"bbbbbbbbbbbb","Names":["/shop-web-1"],"Image":"web","State":"running","Status":"","Labels":{"com.docker.compose.project":"shop","com.docker.compose.service":"web"}}
    ]"#;

    #[tokio::test]
    async fn stopping_a_stack_skips_what_already_stopped_and_reports_what_refused() {
        let fake = fake_engine(vec![
            (200, "application/json", SHOP.to_owned()),
            (
                500,
                "application/json",
                r#"{"message":"cannot stop container: permission denied"}"#.to_owned(),
            ),
        ]);
        let result = act(
            &fake.engine,
            StackParams {
                project: "shop".to_owned(),
                action: StackAction::Stop,
            },
        )
        .await
        .unwrap();
        assert!(result.done.is_empty());
        assert_eq!(result.failed.len(), 1);
        assert_eq!(result.failed[0].name, "shop-web-1");
        assert!(result.failed[0].message.contains("permission denied"));
    }

    #[tokio::test]
    async fn taking_a_stack_down_removes_each_container_but_keeps_its_volumes() {
        let mut fake = fake_engine(vec![
            (200, "application/json", SHOP.to_owned()),
            (204, "text/plain", String::new()),
            (204, "text/plain", String::new()),
        ]);
        let result = act(
            &fake.engine,
            StackParams {
                project: "shop".to_owned(),
                action: StackAction::Down,
            },
        )
        .await
        .unwrap();
        assert_eq!(result.done, ["shop-web-1", "shop-db-1"]);
        fake.seen.recv().await.unwrap();
        let removal = fake.seen.recv().await.unwrap();
        assert!(
            removal.starts_with("DELETE /containers/bbbbbbbbbbbb?force=true&v=false HTTP/1.1"),
            "{removal}"
        );
    }

    #[tokio::test]
    async fn a_project_with_no_containers_is_not_found() {
        let fake = fake_engine(vec![(200, "application/json", "[]".to_owned())]);
        let error = act(
            &fake.engine,
            StackParams {
                project: "gone".to_owned(),
                action: StackAction::Start,
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(error, DockerError::NotFound(_)));
    }
}
