use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct UpdateAgents {
    /// The agents to update; none updates every one with a newer release out.
    #[serde(default)]
    pub kinds: Vec<String>,
}

/// How an update the person asked for is going. One that finished is no longer listed.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum Updating {
    /// Waiting for another agent's update to finish.
    Queued,
    /// The updater runs; `said` is the last line it printed.
    Running {
        said: Option<String>,
    },
    Failed {
        reason: String,
    },
}

fn parts(release: &str) -> (Vec<u64>, Option<&str>) {
    let (numbers, pre) = match release.split_once('-') {
        Some((numbers, pre)) => (numbers, Some(pre)),
        None => (release.split('+').next().unwrap_or(release), None),
    };
    (
        numbers
            .split('.')
            .map(|part| part.parse().unwrap_or(0))
            .collect(),
        pre,
    )
}

/// Whether release `a` comes after release `b`.
pub fn is_newer(a: &str, b: &str) -> bool {
    newer_first(a, b) == std::cmp::Ordering::Less
}

pub fn newer_first(a: &str, b: &str) -> std::cmp::Ordering {
    let ((a_numbers, a_pre), (b_numbers, b_pre)) = (parts(a), parts(b));
    b_numbers.cmp(&a_numbers).then(match (a_pre, b_pre) {
        (None, Some(_)) => std::cmp::Ordering::Less,
        (Some(_), None) => std::cmp::Ordering::Greater,
        (a_pre, b_pre) => b_pre.cmp(&a_pre),
    })
}
