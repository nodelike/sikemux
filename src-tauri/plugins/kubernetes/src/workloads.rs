//! Workloads: Deployments, StatefulSets, DaemonSets, Jobs and CronJobs, each with
//! how much of it is ready and whether something is wrong.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::KubeResult;
use crate::kubectl::{self, Scope};
use crate::pods::text;

#[derive(Debug, Serialize, PartialEq, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Workload {
    /// Deployment, StatefulSet, DaemonSet, Job or CronJob.
    pub kind: String,
    pub name: String,
    pub namespace: String,
    /// How many are ready, for the ones that keep pods running.
    pub ready: Option<u32>,
    /// How many are wanted.
    pub desired: Option<u32>,
    /// ok, progressing, warning or failed.
    pub health: &'static str,
    /// What is wrong or going on, in words, when anything is.
    pub note: Option<String>,
    pub images: Vec<String>,
    pub created: Option<String>,
    /// The label selector its pods carry, such as app=web.
    pub selector: Option<String>,
    /// For a CronJob: when it runs, whether it is paused, and when it last ran.
    pub schedule: Option<String>,
    pub suspended: Option<bool>,
    pub last_run: Option<String>,
}

fn number(value: &Value, path: &str) -> u32 {
    value
        .pointer(path)
        .and_then(Value::as_u64)
        .and_then(|number| u32::try_from(number).ok())
        .unwrap_or(0)
}

fn images(template: &Value) -> Vec<String> {
    let mut found: Vec<String> = template
        .pointer("/spec/containers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|container| text(container, "/image"))
        .collect();
    found.dedup();
    found
}

fn selector_of(value: &Value) -> Option<String> {
    let labels = value.pointer("/spec/selector/matchLabels")?.as_object()?;
    let joined = labels
        .iter()
        .filter_map(|(key, value)| Some(format!("{key}={}", value.as_str()?)))
        .collect::<Vec<_>>()
        .join(",");
    Some(joined).filter(|joined| !joined.is_empty())
}

fn condition<'a>(value: &'a Value, kind: &str) -> Option<&'a Value> {
    value
        .pointer("/status/conditions")
        .and_then(Value::as_array)?
        .iter()
        .find(|condition| text(condition, "/type").as_deref() == Some(kind))
}

fn replicas(ready: u32, desired: u32, stuck: Option<String>) -> (&'static str, Option<String>) {
    if let Some(reason) = stuck {
        return ("failed", Some(reason));
    }
    if desired == 0 {
        return ("ok", Some("Scaled to zero".into()));
    }
    if ready >= desired {
        ("ok", None)
    } else if ready == 0 {
        ("warning", Some(format!("None of {desired} ready")))
    } else {
        ("progressing", Some(format!("{ready} of {desired} ready")))
    }
}

pub fn workload_of(item: &Value) -> Option<Workload> {
    let kind = text(item, "/kind")?;
    let mut workload = Workload {
        name: text(item, "/metadata/name")?,
        namespace: text(item, "/metadata/namespace").unwrap_or_default(),
        kind: kind.clone(),
        ready: None,
        desired: None,
        health: "ok",
        note: None,
        images: images(item.pointer("/spec/template").unwrap_or(&Value::Null)),
        created: text(item, "/metadata/creationTimestamp"),
        selector: selector_of(item),
        schedule: None,
        suspended: None,
        last_run: None,
    };
    match kind.as_str() {
        "Deployment" | "StatefulSet" => {
            let desired = item
                .pointer("/spec/replicas")
                .and_then(Value::as_u64)
                .map_or(1, |n| u32::try_from(n).unwrap_or(1));
            let ready = number(item, "/status/readyReplicas");
            let stuck = condition(item, "Progressing")
                .filter(|progressing| {
                    text(progressing, "/reason").as_deref() == Some("ProgressDeadlineExceeded")
                })
                .map(|_| "The rollout didn't finish in time".to_string());
            let (health, note) = replicas(ready, desired, stuck);
            workload.ready = Some(ready);
            workload.desired = Some(desired);
            workload.health = health;
            workload.note = note;
        }
        "DaemonSet" => {
            let desired = number(item, "/status/desiredNumberScheduled");
            let ready = number(item, "/status/numberReady");
            let (health, note) = replicas(ready, desired, None);
            workload.ready = Some(ready);
            workload.desired = Some(desired);
            workload.health = health;
            workload.note = note;
        }
        "Job" => {
            let completions = item
                .pointer("/spec/completions")
                .and_then(Value::as_u64)
                .map_or(1, |n| u32::try_from(n).unwrap_or(1));
            let succeeded = number(item, "/status/succeeded");
            workload.ready = Some(succeeded);
            workload.desired = Some(completions);
            if let Some(failed) = condition(item, "Failed")
                .filter(|failed| text(failed, "/status").as_deref() == Some("True"))
            {
                workload.health = "failed";
                workload.note = Some(
                    text(failed, "/message")
                        .or_else(|| text(failed, "/reason"))
                        .unwrap_or_else(|| "Failed".into()),
                );
            } else if condition(item, "Complete")
                .is_some_and(|done| text(done, "/status").as_deref() == Some("True"))
            {
                workload.note = Some("Complete".into());
            } else {
                workload.health = "progressing";
                workload.note = Some(format!("{} running", number(item, "/status/active")));
            }
            workload.last_run = text(item, "/status/startTime");
        }
        "CronJob" => {
            workload.images = images(
                item.pointer("/spec/jobTemplate/spec/template")
                    .unwrap_or(&Value::Null),
            );
            workload.schedule = text(item, "/spec/schedule");
            let suspended = item
                .pointer("/spec/suspend")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            workload.suspended = Some(suspended);
            workload.last_run = text(item, "/status/lastScheduleTime");
            let active = item
                .pointer("/status/active")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            workload.note = if suspended {
                Some("Suspended".into())
            } else if active > 0 {
                workload.health = "progressing";
                Some(format!("{active} running"))
            } else {
                None
            };
        }
        _ => return None,
    }
    Some(workload)
}

const ORDER: [&str; 5] = ["Deployment", "StatefulSet", "DaemonSet", "CronJob", "Job"];

/// Workloads with trouble first, then by kind and name.
pub fn workloads_of(list: &Value) -> Vec<Workload> {
    let mut found: Vec<Workload> = list
        .pointer("/items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(workload_of)
        .collect();
    let rank = |workload: &Workload| match workload.health {
        "failed" => 0,
        "warning" => 1,
        "progressing" => 2,
        _ => 3,
    };
    let kind = |workload: &Workload| {
        ORDER
            .iter()
            .position(|each| *each == workload.kind)
            .unwrap_or(ORDER.len())
    };
    found.sort_by(|a, b| {
        (rank(a), kind(a), &a.namespace, &a.name).cmp(&(rank(b), kind(b), &b.namespace, &b.name))
    });
    found
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct ListWorkloads {
    #[serde(flatten)]
    pub scope: Scope,
    pub all_namespaces: bool,
    /// Words in the name.
    pub query: Option<String>,
}

pub async fn list(input: ListWorkloads) -> KubeResult<Vec<Workload>> {
    let mut args = vec!["get", "deployments,statefulsets,daemonsets,cronjobs,jobs"];
    if input.all_namespaces {
        args.push("--all-namespaces");
    }
    let list: Value = kubectl::json(&input.scope, &args).await?;
    let mut found = workloads_of(&list);
    if let Some(words) = input
        .query
        .as_deref()
        .map(str::to_lowercase)
        .filter(|words| !words.trim().is_empty())
    {
        found.retain(|workload| workload.name.contains(&words));
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn deployment(name: &str, ready: u64, desired: u64) -> Value {
        json!({ "kind": "Deployment", "metadata": { "name": name, "namespace": "shop" },
                "spec": { "replicas": desired, "selector": { "matchLabels": { "app": name } },
                          "template": { "spec": { "containers": [{ "image": "nginx:alpine" }] } } },
                "status": { "readyReplicas": ready } })
    }

    #[test]
    fn workloads_read_how_much_is_ready_and_what_is_wrong() {
        let healthy = workload_of(&deployment("web", 2, 2)).unwrap();
        assert_eq!(
            (healthy.health, healthy.ready, healthy.desired),
            ("ok", Some(2), Some(2))
        );
        assert_eq!(healthy.selector.as_deref(), Some("app=web"));
        assert_eq!(healthy.images, ["nginx:alpine"]);
        let crashing = workload_of(&deployment("worker", 0, 1)).unwrap();
        assert_eq!(
            (crashing.health, crashing.note.as_deref()),
            ("warning", Some("None of 1 ready"))
        );
        let mut stuck = deployment("api", 1, 3);
        stuck["status"]["conditions"] = json!([{ "type": "Progressing", "status": "False", "reason": "ProgressDeadlineExceeded" }]);
        assert_eq!(workload_of(&stuck).unwrap().health, "failed");
        assert_eq!(
            workload_of(&deployment("idle", 0, 0))
                .unwrap()
                .note
                .as_deref(),
            Some("Scaled to zero")
        );
    }

    #[test]
    fn jobs_and_cronjobs_read_how_they_ran() {
        let done = json!({ "kind": "Job", "metadata": { "name": "report-now", "namespace": "shop" }, "spec": { "completions": 1 },
            "status": { "succeeded": 1, "startTime": "2026-10-19T08:00:00Z", "conditions": [{ "type": "Complete", "status": "True" }] } });
        let read = workload_of(&done).unwrap();
        assert_eq!(
            (read.health, read.note.as_deref(), read.ready),
            ("ok", Some("Complete"), Some(1))
        );
        let failed = json!({ "kind": "Job", "metadata": { "name": "migrate" }, "status": { "failed": 6,
            "conditions": [{ "type": "Failed", "status": "True", "reason": "BackoffLimitExceeded", "message": "Job has reached the specified backoff limit" }] } });
        assert_eq!(
            workload_of(&failed).unwrap().note.as_deref(),
            Some("Job has reached the specified backoff limit")
        );
        let cron = json!({ "kind": "CronJob", "metadata": { "name": "nightly-report" }, "spec": { "schedule": "0 2 * * *", "suspend": true,
            "jobTemplate": { "spec": { "template": { "spec": { "containers": [{ "image": "busybox:1.36" }] } } } } },
            "status": { "lastScheduleTime": "2026-10-19T02:00:00Z" } });
        let read = workload_of(&cron).unwrap();
        assert_eq!(
            (
                read.schedule.as_deref(),
                read.suspended,
                read.note.as_deref()
            ),
            (Some("0 2 * * *"), Some(true), Some("Suspended"))
        );
        assert_eq!(read.images, ["busybox:1.36"]);
    }

    #[test]
    fn trouble_comes_first_then_kind_and_name() {
        let list = json!({ "items": [deployment("web", 2, 2), deployment("worker", 0, 1),
            { "kind": "DaemonSet", "metadata": { "name": "agent" }, "status": { "desiredNumberScheduled": 1, "numberReady": 1 } }] });
        let names: Vec<String> = workloads_of(&list)
            .into_iter()
            .map(|each| each.name)
            .collect();
        assert_eq!(names, ["worker", "web", "agent"]);
    }
}
