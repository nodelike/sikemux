//! How Rundeck's names read: folders from a job's group, and which folders and
//! projects are production.

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

/// Whether a job in `group` of `project` deploys to production: its top folder
/// or its project says so.
pub fn is_production(project: &str, group: Option<&str>, prod_envs: &[String]) -> bool {
    group_segments(group)
        .first()
        .is_some_and(|top| names_production(top, prod_envs))
        || names_production(project, prod_envs)
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
}
