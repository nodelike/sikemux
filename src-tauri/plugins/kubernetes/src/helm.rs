//! Helm releases: what is installed where, each release's history, the objects it
//! made, and rolling one back. It runs the person's own helm, so it reaches the same
//! clusters kubectl does. Values and notes are never read, since charts put passwords in both.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sikemux_process::{ProcessCancellation, ProcessRunError};

use crate::error::{KubeError, KubeResult};
use crate::kubectl::{self, Scope};
use crate::pods::text;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_OUTPUT_BYTES: usize = 32 * 1024 * 1024;
const MAX_HISTORY: &str = "--max=20";

fn helm_bin() -> String {
    sikemux_process::user_environment::var("HELM").unwrap_or_else(|| "helm".to_string())
}

/// The scope as helm's own flags, which name the context differently from kubectl.
pub fn scope_args(scope: &Scope, all_namespaces: bool) -> KubeResult<Vec<String>> {
    let mut args = Vec::new();
    if let Some(context) = scope
        .context
        .as_deref()
        .filter(|context| !context.trim().is_empty())
    {
        args.push(format!(
            "--kube-context={}",
            kubectl::plain("context", context)?
        ));
    }
    if all_namespaces {
        args.push("--all-namespaces".into());
    } else if let Some(namespace) = scope
        .namespace
        .as_deref()
        .filter(|namespace| !namespace.trim().is_empty())
    {
        args.push(format!(
            "--namespace={}",
            kubectl::object_name("namespace", namespace)?
        ));
    }
    Ok(args)
}

/// The host stops a call by dropping its future, which a blocking run doesn't notice on its own.
struct CancelOnDrop(ProcessCancellation);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

fn run_blocking(args: &[String], cancellation: &ProcessCancellation) -> KubeResult<String> {
    let mut command = sikemux_process::user_environment::command(helm_bin());
    command.args(args);
    let output = sikemux_process::run(
        &mut command,
        None,
        COMMAND_TIMEOUT,
        MAX_OUTPUT_BYTES,
        Some(cancellation),
    )
    .map_err(|error| match &error {
        ProcessRunError::Spawn(cause) if cause.kind() == std::io::ErrorKind::NotFound => {
            KubeError::HelmMissing
        }
        _ => KubeError::Kubectl(error.to_string()),
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(kubectl::classify(stderr.trim_start_matches("Error: ")));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

async fn run(args: Vec<String>) -> KubeResult<String> {
    let cancellation = ProcessCancellation::new();
    let _cancel_on_drop = CancelOnDrop(cancellation.clone());
    tokio::task::spawn_blocking(move || run_blocking(&args, &cancellation))
        .await
        .map_err(|error| KubeError::Kubectl(error.to_string()))?
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Release {
    pub name: String,
    pub namespace: String,
    pub revision: u64,
    pub updated: Option<String>,
    /// deployed, failed, pending-install, pending-upgrade, pending-rollback, superseded or uninstalling.
    pub status: String,
    pub chart: String,
    pub chart_version: Option<String>,
    pub app_version: Option<String>,
}

/// A chart as helm writes it, such as `ingress-nginx-4.11.3` or `app-1.0.0-rc.1`, as its name and version.
pub fn split_chart(chart: &str) -> (String, Option<String>) {
    let mut from = 0;
    while let Some(dash) = chart.get(from..).and_then(|rest| rest.find('-')) {
        let at = from + dash;
        let rest = chart.get(at + 1..).unwrap_or_default();
        if rest.starts_with(|c: char| c.is_ascii_digit()) && rest.contains('.') {
            return (
                chart.get(..at).unwrap_or(chart).to_string(),
                Some(rest.to_string()),
            );
        }
        from = at + 1;
    }
    (chart.to_string(), None)
}

/// Helm 4's `list` writes times as `2026-10-21 11:41:22.588878 +0530 IST`; this writes them as RFC 3339.
pub fn rfc3339(time: &str) -> Option<String> {
    let time = time.trim();
    if time.as_bytes().get(10) == Some(&b'T') {
        return Some(time.to_string());
    }
    let mut parts = time.split_whitespace();
    let date = parts.next()?;
    let clock = parts.next()?.split('.').next()?;
    let offset = parts.next()?;
    let (sign, digits) = offset.split_at_checked(1)?;
    if !matches!(sign, "+" | "-")
        || digits.len() != 4
        || !digits.chars().all(|c| c.is_ascii_digit())
    {
        return None;
    }
    let (hours, minutes) = digits.split_at_checked(2)?;
    Some(format!("{date}T{clock}{sign}{hours}:{minutes}"))
}

/// A release's revision, which helm 4's `list` writes as a string and `history` as a number.
fn revision_of(value: &Value, path: &str) -> u64 {
    match value.pointer(path) {
        Some(Value::Number(number)) => number.as_u64().unwrap_or(0),
        Some(Value::String(text)) => text.parse().unwrap_or(0),
        _ => 0,
    }
}

pub fn release_of(item: &Value) -> Option<Release> {
    let chart = text(item, "/chart").unwrap_or_default();
    let (chart, chart_version) = split_chart(&chart);
    Some(Release {
        name: text(item, "/name")?,
        namespace: text(item, "/namespace").unwrap_or_default(),
        revision: revision_of(item, "/revision"),
        updated: text(item, "/updated").and_then(|updated| rfc3339(&updated)),
        status: text(item, "/status").unwrap_or_else(|| "unknown".into()),
        chart,
        chart_version,
        app_version: text(item, "/app_version"),
    })
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct ListReleases {
    #[serde(flatten)]
    pub scope: Scope,
    pub all_namespaces: bool,
}

pub async fn releases(input: ListReleases) -> KubeResult<Vec<Release>> {
    let mut args = vec!["list".to_string(), "--output=json".to_string()];
    args.extend(scope_args(&input.scope, input.all_namespaces)?);
    let listed: Value = serde_json::from_str(&run(args).await?)?;
    let mut releases: Vec<Release> = listed
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(release_of)
        .collect();
    let trouble = |release: &Release| release.status != "deployed";
    releases.sort_by(|a, b| {
        (!trouble(a), &a.namespace, &a.name).cmp(&(!trouble(b), &b.namespace, &b.name))
    });
    Ok(releases)
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    pub revision: u64,
    pub updated: Option<String>,
    pub status: String,
    pub chart: String,
    pub chart_version: Option<String>,
    pub app_version: Option<String>,
    /// What helm said happened, such as `Upgrade complete` or why it failed.
    pub description: String,
}

pub fn step_of(item: &Value) -> Option<Step> {
    let chart = text(item, "/chart").unwrap_or_default();
    let (chart, chart_version) = split_chart(&chart);
    Some(Step {
        revision: match revision_of(item, "/revision") {
            0 => return None,
            revision => revision,
        },
        updated: text(item, "/updated").and_then(|updated| rfc3339(&updated)),
        status: text(item, "/status").unwrap_or_default(),
        chart,
        chart_version,
        app_version: text(item, "/app_version"),
        description: text(item, "/description").unwrap_or_default(),
    })
}

/// An object a release made, by kind and name only. The manifest's bodies, Secrets
/// included, are read here and never sent on.
#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Made {
    pub kind: String,
    pub name: String,
    pub namespace: Option<String>,
}

fn unquoted(value: &str) -> String {
    value
        .trim()
        .trim_matches(|c| c == '"' || c == '\'')
        .to_string()
}

/// The kind and name of each document in a rendered manifest, read from its top-level
/// `kind` and its `metadata` block only, since other blocks such as `roleRef` have names too.
pub fn made_of(manifest: &str) -> Vec<Made> {
    let mut made = Vec::new();
    for document in manifest.split("\n---") {
        let mut kind = None;
        let mut name = None;
        let mut namespace = None;
        let mut section = "";
        for line in document.lines() {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            if !line.starts_with(' ') {
                section = line.split(':').next().unwrap_or_default();
                if let Some(value) = line.strip_prefix("kind:") {
                    kind = Some(unquoted(value));
                }
                continue;
            }
            if section != "metadata" {
                continue;
            }
            if let Some(value) = line.strip_prefix("  name:") {
                name.get_or_insert_with(|| unquoted(value));
            } else if let Some(value) = line.strip_prefix("  namespace:") {
                namespace.get_or_insert_with(|| unquoted(value));
            }
        }
        if let (Some(kind), Some(name)) = (kind, name) {
            if !kind.is_empty() && !name.is_empty() {
                made.push(Made {
                    kind,
                    name,
                    namespace,
                });
            }
        }
    }
    made
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseDetail {
    /// Newest first.
    pub history: Vec<Step>,
    pub made: Vec<Made>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseRef {
    #[serde(flatten)]
    pub scope: Scope,
    pub name: String,
}

pub async fn release(input: ReleaseRef) -> KubeResult<ReleaseDetail> {
    let name = kubectl::object_name("release", &input.name)?;
    let scope = scope_args(&input.scope, false)?;
    let mut history_args = vec![
        "history".to_string(),
        name.clone(),
        "--output=json".to_string(),
        MAX_HISTORY.to_string(),
    ];
    history_args.extend(scope.iter().cloned());
    let mut manifest_args = vec!["get".to_string(), "manifest".to_string(), name];
    manifest_args.extend(scope);
    let (history, manifest) = tokio::join!(run(history_args), run(manifest_args));
    let history: Value = serde_json::from_str(&history?)?;
    let mut history: Vec<Step> = history
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(step_of)
        .collect();
    history.sort_by_key(|step| std::cmp::Reverse(step.revision));
    Ok(ReleaseDetail {
        history,
        made: manifest
            .map(|manifest| made_of(&manifest))
            .unwrap_or_default(),
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rollback {
    #[serde(flatten)]
    pub release: ReleaseRef,
    pub revision: u64,
}

/// The helm arguments for a rollback, checked first.
pub fn rollback_args(input: &Rollback) -> KubeResult<Vec<String>> {
    let name = kubectl::object_name("release", &input.release.name)?;
    if input.revision == 0 {
        return Err(KubeError::BadArg("say which revision to go back to".into()));
    }
    let mut args = vec!["rollback".to_string(), name, input.revision.to_string()];
    args.extend(scope_args(&input.release.scope, false)?);
    Ok(args)
}

pub async fn rollback(input: Rollback) -> KubeResult<crate::actions::Done> {
    let args = rollback_args(&input)?;
    let said = run(args).await?;
    Ok(crate::actions::Done {
        message: said
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("Rolled back")
            .to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_chart_is_split_into_its_name_and_version() {
        assert_eq!(
            split_chart("demo-chart-0.1.0"),
            ("demo-chart".into(), Some("0.1.0".into()))
        );
        assert_eq!(
            split_chart("ingress-nginx-4.11.3"),
            ("ingress-nginx".into(), Some("4.11.3".into()))
        );
        assert_eq!(
            split_chart("app-v2-1.2.3"),
            ("app-v2".into(), Some("1.2.3".into()))
        );
        assert_eq!(
            split_chart("mychart-1.0.0-rc.1"),
            ("mychart".into(), Some("1.0.0-rc.1".into()))
        );
        assert_eq!(split_chart("plain"), ("plain".into(), None));
    }

    #[test]
    fn helm_4s_two_time_formats_both_read_as_rfc_3339() {
        assert_eq!(
            rfc3339("2026-10-21 11:41:22.588878 +0530 IST").as_deref(),
            Some("2026-10-21T11:41:22+05:30")
        );
        assert_eq!(
            rfc3339("2026-10-21 06:11:22.5 -0700 PDT").as_deref(),
            Some("2026-10-21T06:11:22-07:00")
        );
        assert_eq!(
            rfc3339("2026-10-21T11:40:35.066865+05:30").as_deref(),
            Some("2026-10-21T11:40:35.066865+05:30")
        );
        assert_eq!(rfc3339("yesterday"), None);
    }

    #[test]
    fn a_release_reads_its_revision_whether_helm_wrote_a_string_or_a_number() {
        let listed = release_of(
            &json!({ "name": "storefront", "namespace": "helm-demo", "revision": "3",
            "updated": "2026-10-21 11:41:22.588878 +0530 IST", "status": "failed",
            "chart": "demo-chart-0.1.0", "app_version": "1.16.0" }),
        )
        .expect("reads");
        assert_eq!(listed.revision, 3);
        assert_eq!(listed.chart, "demo-chart");
        assert_eq!(listed.chart_version.as_deref(), Some("0.1.0"));
        assert_eq!(listed.updated.as_deref(), Some("2026-10-21T11:41:22+05:30"));
        let step = step_of(&json!({ "revision": 2, "updated": "2026-10-21T11:41:21.949677+05:30", "status": "deployed",
            "chart": "demo-chart-0.1.0", "app_version": "1.16.0", "description": "two replicas for the sale" }))
        .expect("reads");
        assert_eq!(step.revision, 2);
        assert_eq!(step.description, "two replicas for the sale");
    }

    #[test]
    fn a_manifest_gives_up_kinds_and_names_but_never_a_body() {
        let manifest = "---\n# Source: demo/templates/secret.yaml\napiVersion: v1\nkind: Secret\nmetadata:\n  name: \"demo-db\"\n  namespace: shop\ntype: Opaque\ndata:\n  password: c2VjcmV0\n---\n# Source: demo/templates/rolebinding.yaml\napiVersion: rbac.authorization.k8s.io/v1\nkind: RoleBinding\nmetadata:\n  name: demo-reader\nroleRef:\n  kind: Role\n  name: not-this-one\nsubjects:\n- kind: ServiceAccount\n  name: also-not\n---\napiVersion: apps/v1\nkind: Deployment\nmetadata:\n  labels:\n    name: label-not-name\n  name: demo\nspec:\n  template:\n    metadata:\n      name: nested\n";
        let made = made_of(manifest);
        assert_eq!(
            made,
            [
                Made {
                    kind: "Secret".into(),
                    name: "demo-db".into(),
                    namespace: Some("shop".into())
                },
                Made {
                    kind: "RoleBinding".into(),
                    name: "demo-reader".into(),
                    namespace: None
                },
                Made {
                    kind: "Deployment".into(),
                    name: "demo".into(),
                    namespace: None
                },
            ]
        );
        let sent = serde_json::to_string(&made).expect("writes");
        assert!(!sent.contains("c2VjcmV0"));
        assert!(!sent.contains("password"));
    }

    #[test]
    fn helm_is_pointed_at_the_context_and_namespace_by_its_own_flags() {
        let scope = Scope {
            context: Some("kind-dev".into()),
            namespace: Some("shop".into()),
        };
        assert_eq!(
            scope_args(&scope, false).unwrap(),
            ["--kube-context=kind-dev", "--namespace=shop"]
        );
        assert_eq!(
            scope_args(&scope, true).unwrap(),
            ["--kube-context=kind-dev", "--all-namespaces"]
        );
        let rollback = Rollback {
            release: ReleaseRef {
                scope,
                name: "storefront".into(),
            },
            revision: 2,
        };
        assert_eq!(
            rollback_args(&rollback).unwrap(),
            [
                "rollback",
                "storefront",
                "2",
                "--kube-context=kind-dev",
                "--namespace=shop"
            ]
        );
        let nowhere = Rollback {
            release: ReleaseRef {
                scope: Scope::default(),
                name: "storefront".into(),
            },
            revision: 0,
        };
        assert!(rollback_args(&nowhere).is_err());
        let sneaky = Rollback {
            release: ReleaseRef {
                scope: Scope::default(),
                name: "--dry-run".into(),
            },
            revision: 1,
        };
        assert!(rollback_args(&sneaky).is_err());
    }

    /// Reads the releases on whatever cluster kubectl points at. Reads only.
    #[tokio::test]
    #[ignore = "needs a running cluster with helm"]
    async fn against_the_local_cluster() {
        let all = releases(ListReleases {
            scope: Scope::default(),
            all_namespaces: true,
        })
        .await
        .expect("releases");
        println!("{all:#?}");
        let first = all.first().expect("a release");
        let detail = release(ReleaseRef {
            scope: Scope {
                context: None,
                namespace: Some(first.namespace.clone()),
            },
            name: first.name.clone(),
        })
        .await
        .expect("release");
        println!("{:#?}\n{:#?}", detail.history, detail.made);
        assert!(!detail.history.is_empty());
        assert!(!detail.made.is_empty());
    }
}
