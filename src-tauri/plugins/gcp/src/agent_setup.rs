// The Google account and project agents Sikemux starts work in, so gcloud and
// Google's client libraries in an agent's shell act on the right project
// without the agent hunting for one. A pick per Sikemux project, and one for
// every other project.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use sikemux_plugin_api::PluginContext;
use sikemux_process::user_environment::shell_var;

use crate::error::{GcpError, GcpResult};

const PICKS_FILE: &str = "agent-picks.json";

/// Set in the person's own environment, any of these already says which account
/// or project Google calls use, and Sikemux leaves it be.
const OWN_SETUP: [&str; 5] = [
    "CLOUDSDK_CORE_PROJECT",
    "CLOUDSDK_CORE_ACCOUNT",
    "CLOUDSDK_ACTIVE_CONFIG_NAME",
    "GOOGLE_CLOUD_PROJECT",
    "GOOGLE_APPLICATION_CREDENTIALS",
];

static WRITING: Mutex<()> = Mutex::new(());

#[derive(Serialize, Deserialize, Clone, Default, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Pick {
    #[serde(default)]
    pub account: Option<String>,
    #[serde(default)]
    pub project: Option<String>,
}

impl Pick {
    fn is_empty(&self) -> bool {
        self.account.is_none() && self.project.is_none()
    }

    fn cleaned(self) -> Pick {
        let clean = |value: Option<String>| {
            value
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty() && !v.starts_with('-'))
        };
        Pick {
            account: clean(self.account),
            project: clean(self.project),
        }
    }
}

#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct Picks {
    #[serde(default)]
    default: Option<Pick>,
    #[serde(default)]
    projects: BTreeMap<PathBuf, Pick>,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Source {
    /// The person's own environment, which Sikemux never overrides.
    Environment,
    Project,
    Default,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct Effective {
    pub pick: Pick,
    pub source: Source,
}

/// What the pane shows: the picks for one Sikemux project, and what they come to.
#[derive(Serialize)]
pub struct Picked {
    project: Option<Pick>,
    default: Option<Pick>,
    effective: Option<Effective>,
}

fn effective(
    picks: &Picks,
    folder: Option<&Path>,
    own: impl Fn(&str) -> Option<String>,
) -> Option<Effective> {
    if OWN_SETUP.iter().any(|name| own(name).is_some()) {
        return Some(Effective {
            pick: Pick {
                account: own("CLOUDSDK_CORE_ACCOUNT"),
                project: own("CLOUDSDK_CORE_PROJECT").or_else(|| own("GOOGLE_CLOUD_PROJECT")),
            },
            source: Source::Environment,
        });
    }
    if let Some(pick) = folder.and_then(|folder| picks.projects.get(folder)) {
        return Some(Effective {
            pick: pick.clone(),
            source: Source::Project,
        });
    }
    picks.default.clone().map(|pick| Effective {
        pick,
        source: Source::Default,
    })
}

/// The variables an agent or terminal started in `folder` gets.
pub fn environment_for(picked: Option<&Effective>) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    let Some(picked) = picked.filter(|picked| picked.source != Source::Environment) else {
        return env;
    };
    if let Some(account) = &picked.pick.account {
        env.insert("CLOUDSDK_CORE_ACCOUNT".to_string(), account.clone());
    }
    if let Some(project) = &picked.pick.project {
        env.insert("CLOUDSDK_CORE_PROJECT".to_string(), project.clone());
        env.insert("GOOGLE_CLOUD_PROJECT".to_string(), project.clone());
    }
    env
}

pub fn environment(ctx: &PluginContext, folder: &Path) -> BTreeMap<String, String> {
    let picks = read(ctx.data_dir());
    environment_for(effective(&picks, Some(&canonical(folder)), shell_var).as_ref())
}

pub fn picked(ctx: &PluginContext, folder: Option<&Path>) -> Picked {
    let saved = read(ctx.data_dir());
    let folder = folder.map(canonical);
    Picked {
        effective: effective(&saved, folder.as_deref(), shell_var),
        project: folder.and_then(|folder| saved.projects.get(&folder).cloned()),
        default: saved.default,
    }
}

/// Picks an account and project for agents in `folder`, or for every folder not picked for when there is none.
/// An empty pick clears it.
pub fn pick(ctx: &PluginContext, folder: Option<&Path>, pick: Pick) -> GcpResult<Picked> {
    let pick = pick.cleaned();
    {
        let _turn = WRITING.lock().unwrap_or_else(PoisonError::into_inner);
        let mut saved = read(ctx.data_dir());
        match (folder.map(canonical), pick.is_empty()) {
            (Some(folder), false) => {
                saved.projects.insert(folder, pick);
            }
            (Some(folder), true) => {
                saved.projects.remove(&folder);
            }
            (None, empty) => saved.default = (!empty).then_some(pick),
        }
        let bytes = serde_json::to_vec_pretty(&saved)?;
        sikemux_process::files::write_atomic(&ctx.data_dir().join(PICKS_FILE), &bytes, false)
            .map_err(|error| GcpError::Transport(format!("saving the pick: {error}")))?;
    }
    Ok(picked(ctx, folder))
}

fn read(data_dir: &Path) -> Picks {
    std::fs::read(data_dir.join(PICKS_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Sikemux projects are keyed by their real folder, as the core names them.
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picks() -> Picks {
        Picks {
            default: Some(Pick {
                account: Some("ankit@example.com".into()),
                project: Some("acme-dev".into()),
            }),
            projects: BTreeMap::from([(
                PathBuf::from("/code/shop"),
                Pick {
                    account: None,
                    project: Some("acme-prod".into()),
                },
            )]),
        }
    }

    #[test]
    fn a_folder_pick_beats_the_default_and_the_persons_own_setup_beats_both() {
        let none = |_: &str| None;
        let shop = effective(&picks(), Some(Path::new("/code/shop")), none).expect("a pick");
        assert_eq!(
            (shop.source, shop.pick.project.as_deref()),
            (Source::Project, Some("acme-prod"))
        );
        let other = effective(&picks(), Some(Path::new("/code/blog")), none).expect("a pick");
        assert_eq!(other.source, Source::Default);
        let own = |name: &str| (name == "CLOUDSDK_CORE_PROJECT").then(|| "mine".to_string());
        let theirs = effective(&picks(), Some(Path::new("/code/shop")), own).expect("a pick");
        assert_eq!(
            (theirs.source, theirs.pick.project.as_deref()),
            (Source::Environment, Some("mine"))
        );
        assert!(environment_for(Some(&theirs)).is_empty());
    }

    #[test]
    fn a_pick_becomes_gclouds_and_the_client_libraries_variables() {
        let env =
            environment_for(effective(&picks(), Some(Path::new("/code/blog")), |_| None).as_ref());
        assert_eq!(
            env.get("CLOUDSDK_CORE_ACCOUNT").map(String::as_str),
            Some("ankit@example.com")
        );
        assert_eq!(
            env.get("CLOUDSDK_CORE_PROJECT").map(String::as_str),
            Some("acme-dev")
        );
        assert_eq!(
            env.get("GOOGLE_CLOUD_PROJECT").map(String::as_str),
            Some("acme-dev")
        );
    }

    #[test]
    fn picks_are_saved_and_cleared() {
        let dir = std::env::temp_dir().join(format!("sikemux-gcp-picks-{}", std::process::id()));
        let ctx = PluginContext::new(dir.clone());
        std::fs::create_dir_all(&dir).expect("a folder");
        let picked = pick(
            &ctx,
            None,
            Pick {
                account: None,
                project: Some(" acme-dev ".into()),
            },
        )
        .expect("saved");
        assert_eq!(
            picked.default.and_then(|p| p.project).as_deref(),
            Some("acme-dev")
        );
        let cleared = pick(
            &ctx,
            None,
            Pick {
                account: None,
                project: Some("--flag".into()),
            },
        )
        .expect("cleared");
        assert!(cleared.default.is_none());
        std::fs::remove_dir_all(dir).ok();
    }
}
