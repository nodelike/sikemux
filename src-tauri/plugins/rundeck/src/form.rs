//! The form that starts a job: its options filled in as the run asks, what
//! stops a filled-in form from starting, and what deploying a branch would do.

use std::collections::BTreeMap;

use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use serde::Deserialize;
use sikemux_plugin_api::rundeck::{
    DeployForm, JobOption, OptionKind, Plan, PlanCheck, RunCheck, Tone,
};

use crate::client::Session;
use crate::config::Settings;
use crate::error::RundeckResult;
use crate::executions;
use crate::jobs;
use crate::plan::{self, BranchRelation, PlanResult, PushAction};
use crate::shape::{group_segments, is_live, is_production};
use crate::views::{job_permalink, runs_of, usual_secs};

/// How many of a job's runs its recent branches and usual time come from.
const RECENT_RUNS: u32 = 30;
const RECENT_BRANCHES: usize = 6;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeployRequest {
    job_id: String,
    project: String,
    name: String,
    #[serde(default)]
    group: Option<String>,
    /// The branch to start with.
    #[serde(default)]
    branch: Option<String>,
    /// Option values carried over from an earlier run.
    #[serde(default)]
    given: BTreeMap<String, String>,
}

pub async fn deploy_view(
    session: &Session,
    settings: &Settings,
    request: DeployRequest,
) -> RundeckResult<DeployForm> {
    let branch_options = &settings.branch_options;
    let (detail, history) = futures::join!(
        jobs::job_detail(session, request.job_id.clone()),
        executions::executions(
            session,
            request.job_id.clone(),
            request.project.clone(),
            Some(RECENT_RUNS),
            None,
        )
    );
    let runs = runs_of(&history.unwrap_or_default(), branch_options);
    let (options, description, node_filter, execution_enabled, options_error) = match detail {
        Ok(detail) => (
            detail.options.iter().map(job_option).collect(),
            detail.description,
            detail.node_filter,
            detail.execution_enabled,
            None,
        ),
        Err(error) => (
            given_options(&request.given, branch_options),
            None,
            None,
            true,
            Some(error.to_string()),
        ),
    };
    let branch_key = branch_key(&options, branch_options);
    let values = initial_values(
        &options,
        &request.given,
        branch_key.as_deref(),
        request.branch.as_deref(),
    );
    let mut recent_branches: Vec<String> = Vec::new();
    for branch in runs.iter().filter_map(|run| run.branch.clone()) {
        if recent_branches.len() == RECENT_BRANCHES {
            break;
        }
        if !recent_branches.contains(&branch) {
            recent_branches.push(branch);
        }
    }
    let group = request.group.filter(|group| !group.is_empty());
    let segments = group_segments(group.as_deref());
    Ok(DeployForm {
        permalink: Some(job_permalink(session, &request.project, &request.job_id)),
        prod: is_production(&request.project, group.as_deref(), &settings.prod_envs),
        env: segments
            .first()
            .map_or_else(|| request.project.clone(), |top| (*top).to_owned()),
        service: segments
            .iter()
            .copied()
            .chain([request.name.as_str()])
            .collect::<Vec<_>>()
            .join("/"),
        runs_branch: branch_key.is_some(),
        run_word: if branch_key.is_some() {
            "Deploy"
        } else {
            "Run"
        }
        .to_owned(),
        branch_key,
        job_id: request.job_id,
        name: request.name,
        group,
        project: request.project,
        description,
        node_filter,
        execution_enabled,
        options,
        options_error,
        values,
        usual_secs: usual_secs(&runs),
        recent_branches,
    })
}

fn job_option(option: &jobs::JobOption) -> JobOption {
    JobOption {
        name: option.name.clone(),
        label: option.label.clone(),
        description: option.description.clone(),
        required: option.required,
        secure: option.secure,
        default: option.default.clone(),
        values: option.values.clone(),
        values_url: option.values_url.clone(),
        enforced: option.enforced,
        multivalued: option.multivalued,
        delimiter: option
            .delimiter
            .clone()
            .filter(|delimiter| !delimiter.is_empty())
            .unwrap_or_else(|| ",".to_owned()),
        is_date: option.is_date,
        date_format: option.date_format.clone(),
        kind: match option.kind {
            "file" => OptionKind::File,
            _ => OptionKind::Text,
        },
    }
}

/// When Rundeck cannot hand over the job's definition, as older servers
/// cannot: the branch option alone, and whatever an earlier run was given.
fn given_options(given: &BTreeMap<String, String>, branch_options: &[String]) -> Vec<JobOption> {
    let branch = named_like(given.keys().map(String::as_str), branch_options)
        .or_else(|| branch_options.first().cloned())
        .unwrap_or_else(|| "BRANCH".to_owned());
    let text = |name: &str, required: bool| JobOption {
        name: name.to_owned(),
        label: None,
        description: None,
        required,
        secure: false,
        default: None,
        values: None,
        values_url: None,
        enforced: false,
        multivalued: false,
        delimiter: ",".to_owned(),
        is_date: false,
        date_format: None,
        kind: OptionKind::Text,
    };
    std::iter::once(text(&branch, true))
        .chain(
            given
                .keys()
                .filter(|name| !name.eq_ignore_ascii_case(&branch))
                .map(|name| text(name, false)),
        )
        .collect()
}

/// The first of `names` that `branch_options` names, trying those in order and ignoring case.
fn named_like<'a>(
    names: impl Iterator<Item = &'a str> + Clone,
    branch_options: &[String],
) -> Option<String> {
    branch_options.iter().find_map(|wanted| {
        names
            .clone()
            .find(|name| name.eq_ignore_ascii_case(wanted))
            .map(str::to_owned)
    })
}

fn branch_key(options: &[JobOption], branch_options: &[String]) -> Option<String> {
    named_like(
        options.iter().map(|option| option.name.as_str()),
        branch_options,
    )
}

fn initial_values(
    options: &[JobOption],
    given: &BTreeMap<String, String>,
    branch_key: Option<&str>,
    branch: Option<&str>,
) -> BTreeMap<String, String> {
    let mut values: BTreeMap<String, String> = options
        .iter()
        .map(|option| {
            let value = match option.secure || option.kind == OptionKind::File {
                true => String::new(),
                false => given
                    .get(&option.name)
                    .or(option.default.as_ref())
                    .cloned()
                    .unwrap_or_default(),
            };
            (option.name.clone(), value)
        })
        .collect();
    if let (Some(key), Some(branch)) = (branch_key, branch.filter(|branch| !branch.is_empty())) {
        values.insert(key.to_owned(), branch.to_owned());
    }
    values
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckRequest {
    options: Vec<JobOption>,
    values: BTreeMap<String, String>,
    /// The values read from each option's values URL; none where reading them failed.
    #[serde(default)]
    remote: BTreeMap<String, Option<Vec<String>>>,
    /// Given when the form is about to start the job, to ask whether a run goes on.
    #[serde(default)]
    job_id: Option<String>,
    #[serde(default)]
    project: Option<String>,
}

pub async fn check_run(
    session: &Session,
    settings: &Settings,
    request: CheckRequest,
) -> RundeckResult<RunCheck> {
    let running = match (&request.job_id, &request.project) {
        (Some(job_id), Some(project)) => Some(
            executions::executions(session, job_id.clone(), project.clone(), Some(10), None)
                .await?
                .iter()
                .any(|execution| is_live(execution.status.as_deref())),
        ),
        _ => None,
    };
    let mut errors = option_errors(&request.options, &request.values, &request.remote);
    if let Some(key) = branch_key(&request.options, &settings.branch_options) {
        let branch = request.values.get(&key).map_or("", |value| value.trim());
        if branch.is_empty() {
            errors
                .entry(key)
                .or_insert_with(|| "enter a branch to deploy".to_owned());
        }
    }
    Ok(RunCheck {
        errors,
        send: sent_values(&request.options, &request.values),
        values_urls: request
            .options
            .iter()
            .filter_map(|option| {
                let url = option.values_url.as_deref()?;
                Some((option.name.clone(), fill_option_refs(url, &request.values)))
            })
            .collect(),
        running,
    })
}

fn option_errors(
    options: &[JobOption],
    values: &BTreeMap<String, String>,
    remote: &BTreeMap<String, Option<Vec<String>>>,
) -> BTreeMap<String, String> {
    let mut errors = BTreeMap::new();
    for option in options {
        let value = values.get(&option.name).map_or("", |value| value.trim());
        if option.kind == OptionKind::File {
            if option.required {
                errors.insert(
                    option.name.clone(),
                    "file options must be run from Rundeck".to_owned(),
                );
            }
            continue;
        }
        if value.is_empty() {
            if option.required {
                errors.insert(option.name.clone(), "required".to_owned());
            }
            continue;
        }
        let allowed = match (option.enforced, &option.values_url) {
            (false, _) => None,
            (true, Some(_)) => remote.get(&option.name).cloned().flatten(),
            (true, None) => option.values.clone().filter(|values| !values.is_empty()),
        };
        let Some(allowed) = allowed else { continue };
        let picked: Vec<&str> = match option.multivalued {
            true => split_values(value, &option.delimiter),
            false => vec![value],
        };
        if let Some(bad) = picked
            .iter()
            .find(|entry| !allowed.iter().any(|allowed| allowed == *entry))
        {
            errors.insert(
                option.name.clone(),
                format!("\"{bad}\" is not one of the allowed values"),
            );
        }
    }
    errors
}

fn split_values<'a>(value: &'a str, delimiter: &str) -> Vec<&'a str> {
    value
        .split(delimiter)
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect()
}

/// Every filled-in option but files, which only Rundeck's own form can upload.
fn sent_values(
    options: &[JobOption],
    values: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    options
        .iter()
        .filter(|option| option.kind != OptionKind::File)
        .filter_map(|option| {
            let value = values.get(&option.name)?.trim();
            (!value.is_empty()).then(|| (option.name.clone(), value.to_owned()))
        })
        .collect()
}

/// What a browser's encodeURIComponent leaves as it is.
const URI_COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'!')
    .remove(b'~')
    .remove(b'*')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')');

/// `url` with each `${option.NAME.value}` replaced by that option's value.
fn fill_option_refs(url: &str, values: &BTreeMap<String, String>) -> String {
    const OPEN: &str = "${option.";
    const CLOSE: &str = ".value}";
    let mut out = String::with_capacity(url.len());
    let mut rest = url;
    while let Some((before, after)) = rest.split_once(OPEN) {
        out.push_str(before);
        let reference = after
            .split_once(CLOSE)
            .filter(|(name, _)| !name.is_empty() && !name.contains(['.', '}']));
        match reference {
            Some((name, next)) => {
                let value = values.get(name).map_or("", String::as_str);
                out.extend(utf8_percent_encode(value, URI_COMPONENT));
                rest = next;
            }
            None => {
                out.push_str(OPEN);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanViewRequest {
    job_id: String,
    project: String,
    service: String,
    target_branch: String,
    #[serde(default)]
    repo_path: String,
}

pub async fn plan_view(
    session: &Session,
    settings: &Settings,
    request: PlanViewRequest,
) -> RundeckResult<Plan> {
    let result = plan::plan(
        session,
        plan::PlanRequest {
            job_id: request.job_id,
            project: request.project,
            service: request.service,
            target_branch: request.target_branch,
            repo_path: request.repo_path,
            branch_options: settings.branch_options.clone(),
        },
    )
    .await?;
    Ok(plan_of(result))
}

fn plan_of(plan: PlanResult) -> Plan {
    let check = |tone: Tone, text: String, value: Option<String>| PlanCheck {
        tone,
        text,
        value,
        pushes: false,
    };
    let (tone, text) = match plan.branch_relation {
        BranchRelation::Same => (Tone::Ok, "Same branch as the live one"),
        BranchRelation::TargetContainsDeployed => (Tone::Ok, "Contains everything that is live"),
        BranchRelation::TargetMissingDeployed => (
            Tone::Danger,
            "Doesn't contain the live branch; this is a different line of work",
        ),
        BranchRelation::UnknownNoDeployedBranch => (
            Tone::Info,
            "Nothing deployed successfully yet; this is the first deploy",
        ),
        BranchRelation::UnknownDeployedNotOnOrigin => (
            Tone::Info,
            "The live branch isn't on origin, so they can't be compared",
        ),
        BranchRelation::UnknownTargetNotOnOrigin => (
            Tone::Warn,
            "Not on origin; Rundeck won't find it when it deploys",
        ),
        BranchRelation::UnknownNoRepo => (
            Tone::Info,
            "No local checkout linked, so the branches can't be compared",
        ),
    };
    let mut checks = vec![check(tone, text.to_owned(), None)];
    checks.push(match plan.remote_target_exists {
        true => check(
            Tone::Ok,
            "On origin".to_owned(),
            Some(format!("origin/{}", plan.target_branch)),
        ),
        false => check(Tone::Warn, "Not on origin yet".to_owned(), None),
    });
    if plan.git_root.is_some() {
        checks.push(match &plan.current_branch {
            Some(branch) => check(
                Tone::Info,
                format!("Checkout is on {branch}"),
                plan.head_sha
                    .as_ref()
                    .map(|sha| sha.chars().take(7).collect()),
            ),
            None => check(
                Tone::Warn,
                "Checkout is on a detached HEAD".to_owned(),
                plan.head_sha
                    .as_ref()
                    .map(|sha| sha.chars().take(7).collect()),
            ),
        });
        if plan.dirty {
            checks.push(check(
                Tone::Warn,
                "Checkout has uncommitted changes".to_owned(),
                Some("not deployed".to_owned()),
            ));
        }
    }
    let commits = |count: u32| if count == 1 { "commit" } else { "commits" };
    if let Some(ahead) = plan.ahead.filter(|ahead| *ahead > 0) {
        checks.push(check(
            Tone::Warn,
            format!("{ahead} local {} not pushed", commits(ahead)),
            plan.upstream.clone(),
        ));
    }
    if let Some(behind) = plan.behind.filter(|behind| *behind > 0) {
        checks.push(check(
            Tone::Info,
            format!("{behind} {} behind upstream", commits(behind)),
            plan.upstream.clone(),
        ));
    }
    let pushes = plan.push_action == PushAction::PushCurrent;
    let (tone, text) = match plan.push_action {
        PushAction::PushCurrent => (Tone::Warn, "Pushes your checkout before deploying"),
        PushAction::NotPushDifferentBranch => (
            Tone::Info,
            "Checkout is on another branch; nothing is pushed",
        ),
        PushAction::NotPushNoRepo => (Tone::Info, "No local checkout linked; nothing is pushed"),
        PushAction::NotPushDetached => (
            Tone::Info,
            "Checkout is on a detached HEAD; nothing is pushed",
        ),
    };
    checks.push(PlanCheck {
        tone,
        text: text.to_owned(),
        value: None,
        pushes,
    });
    Plan {
        target_branch: plan.target_branch,
        deployed_branch: plan.deployed_branch,
        current_branch: plan.current_branch,
        pushes,
        checks,
        detail: plan.branch_relation_detail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn option(name: &str) -> JobOption {
        JobOption {
            name: name.to_owned(),
            label: None,
            description: None,
            required: false,
            secure: false,
            default: None,
            values: None,
            values_url: None,
            enforced: false,
            multivalued: false,
            delimiter: ",".to_owned(),
            is_date: false,
            date_format: None,
            kind: OptionKind::Text,
        }
    }

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn prefills_defaults_then_the_previous_run_then_the_branch_and_never_secrets() {
        let options = [
            JobOption {
                default: Some("main".into()),
                ..option("BRANCH")
            },
            JobOption {
                default: Some("eu".into()),
                ..option("REGION")
            },
            JobOption {
                secure: true,
                default: Some("x".into()),
                ..option("PASSWORD")
            },
        ];
        let given = map(&[("REGION", "us"), ("PASSWORD", "leak")]);
        assert_eq!(
            initial_values(&options, &given, Some("BRANCH"), Some("feature/y")),
            map(&[("BRANCH", "feature/y"), ("REGION", "us"), ("PASSWORD", "")])
        );
        assert_eq!(
            initial_values(&options, &BTreeMap::new(), Some("BRANCH"), Some("")),
            map(&[("BRANCH", "main"), ("REGION", "eu"), ("PASSWORD", "")])
        );
    }

    #[test]
    fn fills_option_references_from_the_form() {
        let url = "https://x/values?env=${option.ENV.value}&team=${option.TEAM.value}";
        assert_eq!(
            fill_option_refs(url, &map(&[("ENV", "prod eu"), ("TEAM", "")])),
            "https://x/values?env=prod%20eu&team="
        );
    }

    fn validated() -> Vec<JobOption> {
        vec![
            JobOption {
                required: true,
                ..option("BRANCH")
            },
            JobOption {
                enforced: true,
                values: Some(vec!["s".into(), "m".into()]),
                ..option("SIZE")
            },
            JobOption {
                enforced: true,
                multivalued: true,
                delimiter: "|".into(),
                values: Some(vec!["a".into(), "b".into()]),
                ..option("TAGS")
            },
            JobOption {
                enforced: true,
                values_url: Some("https://x".into()),
                ..option("REMOTE")
            },
            JobOption {
                kind: OptionKind::File,
                required: true,
                ..option("UPLOAD")
            },
        ]
    }

    #[test]
    fn reports_required_enforced_and_file_options_per_field() {
        let remote = BTreeMap::from([("REMOTE".to_owned(), Some(vec!["y".to_owned()]))]);
        let values = map(&[
            ("BRANCH", " "),
            ("SIZE", "xl"),
            ("TAGS", "a|c"),
            ("REMOTE", "z"),
        ]);
        assert_eq!(
            option_errors(&validated(), &values, &remote),
            map(&[
                ("BRANCH", "required"),
                ("SIZE", "\"xl\" is not one of the allowed values"),
                ("TAGS", "\"c\" is not one of the allowed values"),
                ("REMOTE", "\"z\" is not one of the allowed values"),
                ("UPLOAD", "file options must be run from Rundeck"),
            ])
        );
    }

    #[test]
    fn skips_enforcement_when_remote_values_could_not_be_loaded() {
        let remote = BTreeMap::from([("REMOTE".to_owned(), None)]);
        let values = map(&[
            ("BRANCH", "main"),
            ("SIZE", "m"),
            ("TAGS", "a|b"),
            ("REMOTE", "z"),
        ]);
        assert!(option_errors(&validated()[..4], &values, &remote).is_empty());
    }

    #[test]
    fn sends_filled_options_except_files() {
        let options = [
            option("A"),
            option("B"),
            JobOption {
                kind: OptionKind::File,
                ..option("F")
            },
        ];
        assert_eq!(
            sent_values(&options, &map(&[("A", " 1 "), ("B", ""), ("F", "x")])),
            map(&[("A", "1")])
        );
    }
}
