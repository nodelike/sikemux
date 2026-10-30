//! What the person is told when a run they started ends.

use sikemux_plugin_rundeck_views::RunNotice;

use crate::executions::{deployed_branch, Execution};
use crate::shape::{display_status, duration_words, group_segments};

/// A deploy names its branch and where it went; any other job names itself.
pub fn run_notice(execution: &Execution, branch_options: &[String]) -> RunNotice {
    let job = execution.job.as_ref();
    let name = job
        .and_then(|job| job.name.clone())
        .unwrap_or_else(|| format!("Execution #{}", execution.id));
    let place = [
        job.and_then(|job| job.group.clone()),
        job.and_then(|job| job.project.clone()),
        execution.project.clone(),
    ]
    .into_iter()
    .flatten()
    .find(|place| !place.is_empty())
    .unwrap_or_default();
    let branch = deployed_branch(execution, branch_options).filter(|branch| !branch.is_empty());
    let took = execution
        .date_started
        .as_ref()
        .and_then(|date| date.unixtime)
        .zip(execution.date_ended.as_ref().and_then(|date| date.unixtime))
        .map(|(started, ended)| duration_words(ended - started));
    let status = execution
        .status
        .as_deref()
        .unwrap_or_default()
        .to_lowercase();
    let ok = status == "succeeded";
    let to = match place.is_empty() {
        true => String::new(),
        false => format!(" to {place}"),
    };
    let ending = match status.as_str() {
        "failed" => "failed".to_owned(),
        "aborted" => "was aborted".to_owned(),
        "timedout" => "timed out".to_owned(),
        "failed-with-retry" => "failed and will retry".to_owned(),
        _ => format!(
            "ended: {}",
            display_status(
                execution.status.as_deref(),
                execution.custom_status.as_deref()
            )
        ),
    };
    let title = match (ok, &branch) {
        (true, Some(branch)) => format!("Deployed {branch}{to}"),
        (true, None) => format!("{name} finished"),
        (false, Some(branch)) => format!("Deploy of {branch}{to} {ending}"),
        (false, None) if place.is_empty() => format!("{name} {ending}"),
        (false, None) => format!("{name} on {place} {ending}"),
    };
    let body = [
        branch.as_ref().map(|_| name.clone()),
        took.map(|took| match ok {
            true => format!("took {took}"),
            false => format!("after {took}"),
        }),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    RunNotice {
        ok,
        title,
        body: match body.is_empty() {
            true => format!("#{}", execution.id),
            false => body,
        },
    }
}

/// When Rundeck stopped answering before the run ended, so how it went is unknown.
pub fn lost_run_notice(name: &str, group: Option<&str>, execution_id: u64) -> RunNotice {
    let qualified = group_segments(group)
        .into_iter()
        .chain([name])
        .collect::<Vec<_>>()
        .join("/");
    RunNotice {
        ok: false,
        title: format!("Lost track of {qualified}"),
        body: format!(
            "Rundeck stopped answering before run #{execution_id} ended. Check it in Rundeck."
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::executions::{DateField, JobRef};

    const STARTED: i64 = 1_791_000_000_000;

    fn branch_options() -> Vec<String> {
        vec!["branch".to_owned()]
    }

    fn job(name: &str, group: Option<&str>, project: &str, branch: Option<&str>) -> JobRef {
        JobRef {
            id: Some("j1".to_owned()),
            name: Some(name.to_owned()),
            group: group.map(str::to_owned),
            project: Some(project.to_owned()),
            options: Some(
                branch
                    .map(|branch| BTreeMap::from([("branch".to_owned(), branch.to_owned())]))
                    .unwrap_or_default(),
            ),
        }
    }

    fn execution(status: &str, custom: Option<&str>, job: Option<JobRef>) -> Execution {
        let at = |unixtime| DateField {
            date: None,
            unixtime: Some(unixtime),
        };
        Execution {
            id: 4812,
            status: Some(status.to_owned()),
            custom_status: custom.map(str::to_owned),
            user: Some("ankit".to_owned()),
            project: Some("shop".to_owned()),
            date_started: Some(at(STARTED)),
            date_ended: Some(at(STARTED + 252_000)),
            permalink: None,
            job,
            argstring: None,
            workflow_state: None,
        }
    }

    fn deploy(status: &str, custom: Option<&str>) -> RunNotice {
        let job = job("api", Some("staging/backend"), "shop", Some("feature/cart"));
        run_notice(&execution(status, custom, Some(job)), &branch_options())
    }

    #[test]
    fn says_which_branch_went_where_and_how_long_it_took() {
        assert_eq!(
            deploy("succeeded", None),
            RunNotice {
                ok: true,
                title: "Deployed feature/cart to staging/backend".to_owned(),
                body: "api · took 4m 12s".to_owned(),
            }
        );
    }

    #[test]
    fn says_a_deploy_failed_was_aborted_or_timed_out() {
        assert_eq!(
            deploy("failed", None),
            RunNotice {
                ok: false,
                title: "Deploy of feature/cart to staging/backend failed".to_owned(),
                body: "api · after 4m 12s".to_owned(),
            }
        );
        assert_eq!(
            deploy("aborted", None).title,
            "Deploy of feature/cart to staging/backend was aborted"
        );
        assert_eq!(
            deploy("timedout", None).title,
            "Deploy of feature/cart to staging/backend timed out"
        );
        assert_eq!(
            deploy("other", Some("rolled back")).title,
            "Deploy of feature/cart to staging/backend ended: rolled back"
        );
    }

    #[test]
    fn names_a_job_that_takes_no_branch_by_itself() {
        let rotate = || Some(job("rotate-logs", None, "ops", None));
        assert_eq!(
            run_notice(&execution("succeeded", None, rotate()), &branch_options()),
            RunNotice {
                ok: true,
                title: "rotate-logs finished".to_owned(),
                body: "took 4m 12s".to_owned(),
            }
        );
        assert_eq!(
            run_notice(&execution("failed", None, rotate()), &branch_options()).title,
            "rotate-logs on ops failed"
        );
    }

    #[test]
    fn falls_back_to_the_execution_number_when_rundeck_says_little() {
        let bare = Execution {
            project: None,
            date_started: None,
            date_ended: None,
            ..execution("failed", None, None)
        };
        assert_eq!(
            run_notice(&bare, &branch_options()),
            RunNotice {
                ok: false,
                title: "Execution #4812 failed".to_owned(),
                body: "#4812".to_owned(),
            }
        );
    }
}
