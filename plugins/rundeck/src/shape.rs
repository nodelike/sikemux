//! How the pane reads Rundeck's names: folders from a job's group, which
//! folders and projects are production, and how a run's status went.

use sikemux_plugin_rundeck_views::{EnvTone, RunSummary, StatusKind};

use crate::executions::ExecSummary;

/// A job's group as its folders, top first, without empty ones.
pub fn group_segments(group: Option<&str>) -> Vec<&str> {
    group
        .unwrap_or_default()
        .split('/')
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .collect()
}

/// `name` in lower case, then each word of it.
fn env_tokens(name: &str) -> Vec<String> {
    let lower = name.to_lowercase();
    let words: Vec<String> = lower
        .split(|c: char| matches!(c, '-' | '_' | '.' | '/') || c.is_whitespace())
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
        .collect();
    std::iter::once(lower).chain(words).collect()
}

/// Whether `name`, or a word of it, is one of `prod_envs`.
fn names_production(name: &str, prod_envs: &[String]) -> bool {
    env_tokens(name).iter().any(|token| {
        prod_envs
            .iter()
            .any(|env| env.trim().eq_ignore_ascii_case(token))
    })
}

/// Where a job in `group` of `project` runs: its top folder, else its project.
pub fn env_of<'a>(project: &'a str, group: Option<&'a str>) -> &'a str {
    group_segments(group).first().copied().unwrap_or(project)
}

/// The colour of a place a job deploys to; production is whatever the settings name.
pub fn target_tone(project: &str, group: Option<&str>, prod_envs: &[String]) -> EnvTone {
    if is_production(project, group, prod_envs) {
        return EnvTone::Prod;
    }
    let tokens = env_tokens(env_of(project, group));
    let any = |test: fn(&str) -> bool| tokens.iter().any(|token| test(token));
    if any(|t| t.starts_with("stag") || matches!(t, "stg" | "qa" | "uat")) {
        EnvTone::Staging
    } else if any(|t| t.starts_with("pre")) {
        EnvTone::Preprod
    } else if any(|t| t.starts_with("dev") || matches!(t, "local" | "test")) {
        EnvTone::Dev
    } else {
        EnvTone::Other
    }
}

/// A run's length as the pane writes it: `4.2s`, `4m 12s`, `1h 5m`.
pub fn duration_words(ms: i64) -> String {
    let ms = ms.max(0);
    if ms == 0 {
        return "0s".to_owned();
    }
    if ms < 1000 {
        return format!("{ms}ms");
    }
    if ms < 10_000 {
        let tenths = (ms + 50) / 100;
        return match tenths % 10 {
            0 => format!("{}s", tenths / 10),
            rest => format!("{}.{rest}s", tenths / 10),
        };
    }
    let seconds = (ms + 500) / 1000;
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return match seconds % 60 {
            0 => format!("{minutes}m"),
            rest => format!("{minutes}m {rest}s"),
        };
    }
    match minutes % 60 {
        0 => format!("{}h", minutes / 60),
        rest => format!("{}h {rest}m", minutes / 60),
    }
}

/// Whether a job in `group` of `project` deploys to production: its top folder
/// or its project says so.
pub fn is_production(project: &str, group: Option<&str>, prod_envs: &[String]) -> bool {
    group_segments(group)
        .first()
        .is_some_and(|top| names_production(top, prod_envs))
        || names_production(project, prod_envs)
}

pub fn is_live(status: Option<&str>) -> bool {
    status.is_some_and(|status| {
        matches!(
            status.to_ascii_lowercase().as_str(),
            "running" | "scheduled" | "queued"
        )
    })
}

pub fn status_kind(status: Option<&str>) -> StatusKind {
    let Some(status) = status else {
        return StatusKind::Unknown;
    };
    match status.to_ascii_lowercase().as_str() {
        "succeeded" => StatusKind::Succeeded,
        "failed" | "failed-with-retry" | "timedout" | "other-failed" | "error" => {
            StatusKind::Failed
        }
        "aborted" => StatusKind::Aborted,
        live if is_live(Some(live)) => StatusKind::Running,
        _ => StatusKind::Unknown,
    }
}

/// What a status reads as: Rundeck's own custom status where it set one.
pub fn display_status(status: Option<&str>, custom: Option<&str>) -> String {
    match custom {
        Some(custom) if status.is_none_or(|status| status.eq_ignore_ascii_case("other")) => {
            custom.to_owned()
        }
        _ => status.unwrap_or("unknown").to_owned(),
    }
}

pub fn run_summary(run: &ExecSummary) -> RunSummary {
    RunSummary {
        execution_id: run.execution_id,
        status: display_status(run.status.as_deref(), run.custom_status.as_deref()),
        status_kind: status_kind(run.status.as_deref()),
        live: is_live(run.status.as_deref()),
        user: run.user.clone(),
        started_at: run.started_at.clone(),
        ended_at: run.ended_at.clone(),
        branch: run.branch.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prod() -> Vec<String> {
        ["prod", "production", "prd", "live"]
            .map(String::from)
            .to_vec()
    }

    #[test]
    fn production_matches_whole_words_not_prefixes() {
        for name in ["prod", "PROD", "eu-prod", "live_payments", "production"] {
            assert!(names_production(name, &prod()), "{name}");
        }
        for name in ["preprod", "product-catalog", "delivery"] {
            assert!(!names_production(name, &prod()), "{name}");
        }
    }

    #[test]
    fn production_checks_both_the_top_folder_and_the_project() {
        assert!(is_production("Deployments", Some("prod/api"), &prod()));
        assert!(is_production("payments-prod", Some("backend/api"), &prod()));
        assert!(!is_production("Deployments", Some("staging/api"), &prod()));
        assert!(is_production(
            "Deployments",
            Some("staging/api"),
            &["staging".to_owned()]
        ));
    }

    #[test]
    fn colours_places_outside_production() {
        let tone = |group: &str| target_tone("ops", Some(group), &prod());
        assert_eq!(tone("staging/api"), EnvTone::Staging);
        assert_eq!(tone("preprod/api"), EnvTone::Preprod);
        assert_eq!(tone("dev/api"), EnvTone::Dev);
        assert_eq!(tone("tools/api"), EnvTone::Other);
        assert_eq!(tone("prod/api"), EnvTone::Prod);
    }

    #[test]
    fn treats_queued_and_scheduled_runs_as_live() {
        assert!(is_live(Some("running")));
        assert!(is_live(Some("Scheduled")));
        assert!(is_live(Some("queued")));
        assert!(!is_live(Some("succeeded")));
        assert!(!is_live(None));
    }

    #[test]
    fn shows_a_custom_status_in_place_of_other() {
        assert_eq!(
            display_status(Some("other"), Some("deployed-with-warnings")),
            "deployed-with-warnings"
        );
        assert_eq!(display_status(Some("failed"), Some("ignored")), "failed");
        assert_eq!(display_status(None, None), "unknown");
    }

    #[test]
    fn writes_durations_as_the_pane_does() {
        assert_eq!(duration_words(252_000), "4m 12s");
        assert_eq!(duration_words(4_200), "4.2s");
        assert_eq!(duration_words(3_900_000), "1h 5m");
    }
}
