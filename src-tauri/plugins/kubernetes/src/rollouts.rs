//! A workload's rollout history: a Deployment's revisions from the ReplicaSets it
//! made, and a StatefulSet's or DaemonSet's from its ControllerRevisions, each with
//! the images it ran, why it was made, and which one is live.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{KubeError, KubeResult};
use crate::kubectl::{self, Scope};
use crate::pods::text;

const REVISION: &str = "deployment.kubernetes.io/revision";
const CHANGE_CAUSE: &str = "kubernetes.io/change-cause";
/// `kubectl rollout restart` stamps the template with this, so a revision that only restarted can say so.
const RESTARTED_AT: &str = "kubectl.kubernetes.io/restartedAt";
/// Left by `kubectl apply`, which a rollback doesn't update, so the next apply of the old file undoes it.
const LAST_APPLIED: &str = "kubectl.kubernetes.io/last-applied-configuration";

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Revision {
    pub revision: u64,
    /// The ReplicaSet or ControllerRevision that holds it.
    pub name: String,
    pub current: bool,
    pub change_cause: Option<String>,
    pub images: Vec<String>,
    /// When the template was last restarted, which every later revision carries too.
    #[serde(skip)]
    pub restarted_at: Option<String>,
    /// Made by a restart: the same images as the revision before, with a newer restart stamp.
    pub restart: bool,
    pub created: Option<String>,
    /// Pods running this revision, for a Deployment.
    pub pods: Option<u64>,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct History {
    pub kind: String,
    pub name: String,
    /// Only a Deployment's rollout can be paused.
    pub paused: bool,
    /// Made with `kubectl apply`: undoing here leaves its saved file behind, so the next apply reverts the undo.
    pub applied: bool,
    pub current: Option<u64>,
    /// Newest first.
    pub revisions: Vec<Revision>,
}

fn annotation(item: &Value, key: &str) -> Option<String> {
    item.pointer("/metadata/annotations")
        .and_then(|annotations| annotations.get(key))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn images_of(template: &Value) -> Vec<String> {
    template
        .pointer("/spec/containers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|container| text(container, "/image"))
        .collect()
}

fn owned_by(item: &Value, uid: &str) -> bool {
    item.pointer("/metadata/ownerReferences")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|owner| text(owner, "/uid").as_deref() == Some(uid))
}

/// Marks the revisions a restart made: a restart changes only the stamp, never the
/// images, and the stamp then rides along on every revision after it.
fn mark_restarts(revisions: &mut [Revision]) {
    for index in 0..revisions.len() {
        let (newer, older) = revisions.split_at_mut(index + 1);
        let (Some(revision), Some(before)) = (newer.last_mut(), older.first()) else {
            continue;
        };
        revision.restart = revision.restarted_at.is_some()
            && revision.restarted_at != before.restarted_at
            && revision.images == before.images;
    }
}

/// A Deployment's history from the ReplicaSets it owns.
pub fn deployment_history(deployment: &Value, replica_sets: &Value) -> Option<History> {
    let name = text(deployment, "/metadata/name")?;
    let uid = text(deployment, "/metadata/uid")?;
    let current = annotation(deployment, REVISION).and_then(|revision| revision.parse().ok());
    let mut revisions: Vec<Revision> = replica_sets
        .pointer("/items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|set| owned_by(set, &uid))
        .filter_map(|set| {
            let revision: u64 = annotation(set, REVISION)?.parse().ok()?;
            let template = set.pointer("/spec/template").unwrap_or(&Value::Null);
            Some(Revision {
                revision,
                name: text(set, "/metadata/name").unwrap_or_default(),
                current: Some(revision) == current,
                change_cause: annotation(set, CHANGE_CAUSE),
                images: images_of(template),
                restarted_at: annotation(template, RESTARTED_AT),
                restart: false,
                created: text(set, "/metadata/creationTimestamp"),
                pods: Some(
                    set.pointer("/status/replicas")
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                ),
            })
        })
        .collect();
    revisions.sort_by_key(|revision| std::cmp::Reverse(revision.revision));
    mark_restarts(&mut revisions);
    Some(History {
        kind: "Deployment".into(),
        name,
        paused: deployment
            .pointer("/spec/paused")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        applied: annotation(deployment, LAST_APPLIED).is_some(),
        current,
        revisions,
    })
}

/// A StatefulSet's or DaemonSet's history from the ControllerRevisions it owns.
pub fn controller_history(
    kind: &str,
    workload: &Value,
    controller_revisions: &Value,
) -> Option<History> {
    let name = text(workload, "/metadata/name")?;
    let uid = text(workload, "/metadata/uid")?;
    let live = text(workload, "/status/currentRevision");
    let mut revisions: Vec<Revision> = controller_revisions
        .pointer("/items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|revision| owned_by(revision, &uid))
        .filter_map(|revision| {
            let number = revision.pointer("/revision").and_then(Value::as_u64)?;
            let template = revision
                .pointer("/data/spec/template")
                .unwrap_or(&Value::Null);
            Some(Revision {
                revision: number,
                name: text(revision, "/metadata/name").unwrap_or_default(),
                current: false,
                change_cause: annotation(revision, CHANGE_CAUSE),
                images: images_of(template),
                restarted_at: annotation(template, RESTARTED_AT),
                restart: false,
                created: text(revision, "/metadata/creationTimestamp"),
                pods: None,
            })
        })
        .collect();
    revisions.sort_by_key(|revision| std::cmp::Reverse(revision.revision));
    mark_restarts(&mut revisions);
    let current = match live {
        Some(live) => revisions
            .iter()
            .find(|revision| revision.name == live)
            .map(|revision| revision.revision),
        None => revisions.first().map(|revision| revision.revision),
    };
    for revision in &mut revisions {
        revision.current = Some(revision.revision) == current;
    }
    Some(History {
        kind: kind.to_string(),
        name,
        paused: false,
        applied: annotation(workload, LAST_APPLIED).is_some(),
        current,
        revisions,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryOf {
    #[serde(flatten)]
    pub scope: Scope,
    /// Deployment, StatefulSet or DaemonSet.
    pub kind: String,
    pub name: String,
}

/// The resource kubectl knows a workload kind by, for the kinds that keep history.
pub fn resource_of(kind: &str) -> KubeResult<&'static str> {
    match kind.to_lowercase().as_str() {
        "deployment" | "deployments" | "deploy" => Ok("deployment"),
        "statefulset" | "statefulsets" | "sts" => Ok("statefulset"),
        "daemonset" | "daemonsets" | "ds" => Ok("daemonset"),
        other => Err(KubeError::BadArg(format!(
            "a {other} keeps no rollout history; only Deployments, StatefulSets and DaemonSets do"
        ))),
    }
}

pub async fn history(input: HistoryOf) -> KubeResult<History> {
    let resource = resource_of(&input.kind)?;
    let name = kubectl::object_name("name", &input.name)?;
    let workload: Value = kubectl::json(&input.scope, &["get", resource, &name]).await?;
    if resource == "deployment" {
        let selector = selector_of(&workload)?;
        let sets: Value = kubectl::json(
            &input.scope,
            &["get", "replicasets", "--selector", &selector],
        )
        .await?;
        return deployment_history(&workload, &sets)
            .ok_or_else(|| KubeError::Response("the Deployment came back without a name".into()));
    }
    let revisions: Value = kubectl::json(&input.scope, &["get", "controllerrevisions"]).await?;
    let kind = if resource == "statefulset" {
        "StatefulSet"
    } else {
        "DaemonSet"
    };
    controller_history(kind, &workload, &revisions)
        .ok_or_else(|| KubeError::Response("the workload came back without a name".into()))
}

/// A workload's label selector as `kubectl --selector` takes it.
pub fn selector_of(workload: &Value) -> KubeResult<String> {
    let labels = workload
        .pointer("/spec/selector/matchLabels")
        .and_then(Value::as_object)
        .ok_or_else(|| KubeError::Response("the workload has no label selector".into()))?;
    let mut pairs: Vec<String> = labels
        .iter()
        .filter_map(|(key, value)| Some(format!("{key}={}", value.as_str()?)))
        .collect();
    pairs.sort();
    if pairs.is_empty() {
        return Err(KubeError::Response(
            "the workload has no label selector".into(),
        ));
    }
    Ok(pairs.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn deployment() -> Value {
        json!({
            "metadata": { "name": "web", "uid": "d-1", "annotations": { REVISION: "4" } },
            "spec": { "paused": false, "selector": { "matchLabels": { "app": "web", "tier": "front" } } }
        })
    }

    fn replica_set(
        revision: u64,
        image: &str,
        cause: Option<&str>,
        restarted: Option<&str>,
        pods: u64,
        owner: &str,
    ) -> Value {
        let mut annotations = json!({ REVISION: revision.to_string() });
        if let Some(cause) = cause {
            annotations[CHANGE_CAUSE] = json!(cause);
        }
        let template_annotations = restarted.map_or(json!({}), |at| json!({ RESTARTED_AT: at }));
        json!({
            "metadata": { "name": format!("web-{revision}"), "annotations": annotations,
                          "creationTimestamp": format!("2026-10-2{revision}T06:00:00Z"),
                          "ownerReferences": [{ "kind": "Deployment", "uid": owner }] },
            "spec": { "template": { "metadata": { "annotations": template_annotations },
                                    "spec": { "containers": [{ "image": image }] } } },
            "status": { "replicas": pods }
        })
    }

    #[test]
    fn a_deployments_history_comes_from_its_own_replica_sets_newest_first() {
        let sets = json!({ "items": [
            replica_set(1, "nginx:alpine", None, None, 0, "d-1"),
            replica_set(2, "nginx:alpine", None, Some("2026-10-19T11:54:04Z"), 0, "d-1"),
            replica_set(4, "nginx:1.27.3-alpine", Some("pin nginx 1.27.3"), Some("2026-10-19T11:54:04Z"), 2, "d-1"),
            replica_set(3, "nginx:1.27-alpine", Some("move to 1.27"), Some("2026-10-19T11:54:04Z"), 0, "d-1"),
            replica_set(9, "other:1", None, None, 1, "someone-else"),
        ] });
        let history = deployment_history(&deployment(), &sets).expect("reads");
        let numbers: Vec<u64> = history
            .revisions
            .iter()
            .map(|revision| revision.revision)
            .collect();
        assert_eq!(numbers, [4, 3, 2, 1]);
        assert_eq!(history.current, Some(4));
        assert!(history.revisions[0].current);
        assert!(!history.revisions[1].current);
        assert_eq!(history.revisions[0].pods, Some(2));
        assert_eq!(
            history.revisions[1].change_cause.as_deref(),
            Some("move to 1.27")
        );
        let restarts: Vec<bool> = history
            .revisions
            .iter()
            .map(|revision| revision.restart)
            .collect();
        assert_eq!(restarts, [false, false, true, false]);
        assert_eq!(history.revisions[0].images, ["nginx:1.27.3-alpine"]);
        assert!(!history.applied);
        let mut applied = deployment();
        applied["metadata"]["annotations"][LAST_APPLIED] = json!("{}");
        assert!(deployment_history(&applied, &sets).expect("reads").applied);
    }

    #[test]
    fn a_statefulsets_history_marks_the_revision_its_pods_run() {
        let workload = json!({ "metadata": { "name": "cache", "uid": "s-1" }, "status": { "currentRevision": "cache-1" } });
        let revisions = json!({ "items": [
            { "revision": 1, "metadata": { "name": "cache-1", "ownerReferences": [{ "uid": "s-1" }] },
              "data": { "spec": { "template": { "spec": { "containers": [{ "image": "busybox:1.36" }] } } } } },
            { "revision": 2, "metadata": { "name": "cache-2", "ownerReferences": [{ "uid": "s-1" }] },
              "data": { "spec": { "template": { "spec": { "containers": [{ "image": "busybox:1.37" }] } } } } }
        ] });
        let history = controller_history("StatefulSet", &workload, &revisions).expect("reads");
        assert_eq!(history.current, Some(1));
        assert!(!history.revisions[0].current && history.revisions[1].current);
        assert_eq!(history.revisions[0].images, ["busybox:1.37"]);
        let daemon = json!({ "metadata": { "name": "agent", "uid": "s-1" } });
        assert_eq!(
            controller_history("DaemonSet", &daemon, &revisions)
                .expect("reads")
                .current,
            Some(2)
        );
    }

    #[test]
    fn only_kinds_that_keep_history_are_asked_for_it() {
        assert_eq!(resource_of("Deployment").ok(), Some("deployment"));
        assert_eq!(resource_of("sts").ok(), Some("statefulset"));
        assert_eq!(resource_of("DaemonSet").ok(), Some("daemonset"));
        assert!(resource_of("CronJob").is_err());
        assert_eq!(
            selector_of(&deployment()).ok().as_deref(),
            Some("app=web,tier=front")
        );
        assert!(selector_of(&json!({ "spec": {} })).is_err());
    }

    /// Reads the shop namespace's history on whatever cluster kubectl points at. Reads only.
    #[tokio::test]
    #[ignore = "needs a running cluster"]
    async fn against_the_local_cluster() {
        let scope = Scope {
            context: None,
            namespace: Some("shop".into()),
        };
        let web = history(HistoryOf {
            scope: scope.clone(),
            kind: "Deployment".into(),
            name: "web".into(),
        })
        .await
        .expect("web");
        println!("{web:#?}");
        assert!(web.revisions.len() > 1);
        assert_eq!(
            web.revisions
                .iter()
                .filter(|revision| revision.current)
                .count(),
            1
        );
        let cache = history(HistoryOf {
            scope,
            kind: "StatefulSet".into(),
            name: "cache".into(),
        })
        .await
        .expect("cache");
        println!("{cache:#?}");
        assert_eq!(
            cache
                .revisions
                .iter()
                .filter(|revision| revision.current)
                .count(),
            1
        );
    }
}
