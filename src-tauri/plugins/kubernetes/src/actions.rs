//! Changing workloads: restarting, scaling, replacing a pod, and running or pausing a CronJob.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::{KubeError, KubeResult};
use crate::kubectl::{self, Scope};

/// The most replicas Sikemux scales a workload to, so a slip of the keyboard can't flood a cluster.
const MAX_REPLICAS: u32 = 50;

#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Action {
    /// Replace every pod of a Deployment, StatefulSet or DaemonSet, one after another.
    Restart,
    /// Set how many pods a Deployment or StatefulSet runs.
    Scale,
    /// Delete one pod, which its workload then replaces.
    DeletePod,
    /// Start a CronJob's job now, outside its schedule.
    RunNow,
    Suspend,
    Resume,
    /// Roll a Deployment, StatefulSet or DaemonSet back to one of its revisions.
    Undo,
    /// Stop a Deployment's rollout where it is, so changes wait.
    PauseRollout,
    ResumeRollout,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Act {
    #[serde(flatten)]
    pub scope: Scope,
    pub action: Action,
    /// Deployment, StatefulSet, DaemonSet, CronJob or Pod.
    #[serde(default)]
    pub kind: Option<String>,
    pub name: String,
    #[serde(default)]
    pub replicas: Option<u32>,
    /// The revision to undo to.
    #[serde(default)]
    pub revision: Option<u64>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Done {
    /// What happened, in words.
    pub message: String,
}

fn resource(kind: Option<&str>, allowed: &[&str]) -> KubeResult<&'static str> {
    let kind = kind.unwrap_or("Deployment").to_lowercase();
    let found = match kind.as_str() {
        "deployment" | "deployments" | "deploy" => "deployment",
        "statefulset" | "statefulsets" | "sts" => "statefulset",
        "daemonset" | "daemonsets" | "ds" => "daemonset",
        "cronjob" | "cronjobs" | "cj" => "cronjob",
        _ => {
            return Err(KubeError::BadArg(format!(
                "{kind} can't be changed this way"
            )))
        }
    };
    if !allowed.contains(&found) {
        return Err(KubeError::BadArg(format!("a {found} can't do that")));
    }
    Ok(found)
}

/// The kubectl arguments for an action, checked first.
pub fn args_of(input: &Act, now_secs: u64) -> KubeResult<Vec<String>> {
    let name = kubectl::object_name("name", &input.name)?;
    let kind = input.kind.as_deref();
    Ok(match input.action {
        Action::Restart => {
            let kind = resource(kind, &["deployment", "statefulset", "daemonset"])?;
            vec!["rollout".into(), "restart".into(), format!("{kind}/{name}")]
        }
        Action::Scale => {
            let kind = resource(kind, &["deployment", "statefulset"])?;
            let replicas = input
                .replicas
                .ok_or_else(|| KubeError::BadArg("say how many replicas".into()))?;
            if replicas > MAX_REPLICAS {
                return Err(KubeError::BadArg(format!(
                    "Sikemux scales to at most {MAX_REPLICAS}; use kubectl for more"
                )));
            }
            vec![
                "scale".into(),
                format!("{kind}/{name}"),
                format!("--replicas={replicas}"),
            ]
        }
        Action::DeletePod => vec!["delete".into(), "pod".into(), name, "--wait=false".into()],
        Action::RunNow => {
            resource(kind.or(Some("cronjob")), &["cronjob"])?;
            let job: String = format!("{name}-manual-{now_secs}")
                .chars()
                .take(63)
                .collect();
            vec![
                "create".into(),
                "job".into(),
                job,
                format!("--from=cronjob/{name}"),
            ]
        }
        Action::Undo => {
            let kind = resource(kind, &["deployment", "statefulset", "daemonset"])?;
            let revision = input
                .revision
                .filter(|revision| *revision > 0)
                .ok_or_else(|| KubeError::BadArg("say which revision to go back to".into()))?;
            vec![
                "rollout".into(),
                "undo".into(),
                format!("{kind}/{name}"),
                format!("--to-revision={revision}"),
            ]
        }
        Action::PauseRollout | Action::ResumeRollout => {
            let kind = resource(kind, &["deployment"])?;
            let verb = if input.action == Action::PauseRollout {
                "pause"
            } else {
                "resume"
            };
            vec!["rollout".into(), verb.into(), format!("{kind}/{name}")]
        }
        Action::Suspend | Action::Resume => {
            resource(kind.or(Some("cronjob")), &["cronjob"])?;
            let suspend = input.action == Action::Suspend;
            vec![
                "patch".into(),
                "cronjob".into(),
                name,
                "--type=merge".into(),
                format!("--patch={{\"spec\":{{\"suspend\":{suspend}}}}}"),
            ]
        }
    })
}

pub async fn act(input: Act) -> KubeResult<Done> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let args = args_of(&input, now)?;
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let said = kubectl::run(&input.scope, &refs).await?;
    Ok(Done {
        message: said.trim().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn act(json: serde_json::Value) -> Act {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn each_action_asks_kubectl_for_just_that() {
        let restart = args_of(
            &act(serde_json::json!({ "action": "restart", "kind": "Deployment", "name": "web" })),
            0,
        )
        .unwrap();
        assert_eq!(restart, ["rollout", "restart", "deployment/web"]);
        let scale = args_of(&act(serde_json::json!({ "action": "scale", "kind": "sts", "name": "db", "replicas": 3 })), 0).unwrap();
        assert_eq!(scale, ["scale", "statefulset/db", "--replicas=3"]);
        let delete = args_of(
            &act(serde_json::json!({ "action": "deletePod", "name": "web-1" })),
            0,
        )
        .unwrap();
        assert_eq!(delete, ["delete", "pod", "web-1", "--wait=false"]);
        let run = args_of(
            &act(serde_json::json!({ "action": "runNow", "name": "nightly-report" })),
            1_792_400_000,
        )
        .unwrap();
        assert_eq!(
            run,
            [
                "create",
                "job",
                "nightly-report-manual-1792400000",
                "--from=cronjob/nightly-report"
            ]
        );
        let pause = args_of(&act(serde_json::json!({ "action": "suspend", "kind": "CronJob", "name": "nightly-report" })), 0).unwrap();
        assert_eq!(
            pause.last().map(String::as_str),
            Some("--patch={\"spec\":{\"suspend\":true}}")
        );
    }

    #[test]
    fn a_rollout_is_undone_to_a_named_revision_and_paused_only_for_a_deployment() {
        let undo = args_of(&act(serde_json::json!({ "action": "undo", "kind": "Deployment", "name": "web", "revision": 3 })), 0).unwrap();
        assert_eq!(
            undo,
            ["rollout", "undo", "deployment/web", "--to-revision=3"]
        );
        let sts = args_of(&act(serde_json::json!({ "action": "undo", "kind": "sts", "name": "cache", "revision": 1 })), 0).unwrap();
        assert_eq!(
            sts,
            ["rollout", "undo", "statefulset/cache", "--to-revision=1"]
        );
        assert!(args_of(
            &act(serde_json::json!({ "action": "undo", "kind": "Deployment", "name": "web" })),
            0
        )
        .is_err());
        assert!(args_of(&act(serde_json::json!({ "action": "undo", "kind": "Deployment", "name": "web", "revision": 0 })), 0).is_err());
        let pause = args_of(&act(serde_json::json!({ "action": "pauseRollout", "kind": "Deployment", "name": "web" })), 0).unwrap();
        assert_eq!(pause, ["rollout", "pause", "deployment/web"]);
        let resume = args_of(
            &act(serde_json::json!({ "action": "resumeRollout", "name": "web" })),
            0,
        )
        .unwrap();
        assert_eq!(resume, ["rollout", "resume", "deployment/web"]);
        assert!(args_of(&act(serde_json::json!({ "action": "pauseRollout", "kind": "StatefulSet", "name": "cache" })), 0).is_err());
    }

    #[test]
    fn what_a_kind_can_t_do_is_turned_away() {
        assert!(args_of(&act(serde_json::json!({ "action": "scale", "kind": "DaemonSet", "name": "agent", "replicas": 2 })), 0).is_err());
        assert!(args_of(
            &act(serde_json::json!({ "action": "scale", "name": "web" })),
            0
        )
        .is_err());
        assert!(args_of(
            &act(serde_json::json!({ "action": "scale", "name": "web", "replicas": 500 })),
            0
        )
        .is_err());
        assert!(args_of(
            &act(serde_json::json!({ "action": "restart", "kind": "Secret", "name": "web" })),
            0
        )
        .is_err());
        assert!(args_of(
            &act(serde_json::json!({ "action": "runNow", "kind": "Deployment", "name": "web" })),
            0
        )
        .is_err());
        assert!(args_of(
            &act(serde_json::json!({ "action": "deletePod", "name": "--all" })),
            0
        )
        .is_err());
    }
}
