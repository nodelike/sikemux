// The profile every agent Sikemux starts gets as AWS_PROFILE, so `aws` works
// in it without the agent hunting through ~/.aws/config.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use sikemux_plugin_api::PluginContext;
use sikemux_process::sync::Locked;
use sikemux_process::user_environment::shell_var;

use crate::auth;
use crate::error::AwsResult;

const PICKS_FILE: &str = "agent-profiles.json";

/// Set in the person's own environment, any of these already says who AWS
/// calls run as, by a profile, keys, or a tool such as aws-vault or Granted.
const OWN_IDENTITY: [&str; 5] = [
    "AWS_PROFILE",
    "AWS_DEFAULT_PROFILE",
    "AWS_ACCESS_KEY_ID",
    "AWS_VAULT",
    "AWS_WEB_IDENTITY_TOKEN_FILE",
];

static WRITING: Mutex<()> = Mutex::new(());

/// The profiles the person picked for agents: one per project, and one for
/// every other project.
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Picks {
    #[serde(default)]
    default: Option<String>,
    #[serde(default)]
    projects: BTreeMap<PathBuf, String>,
    /// The profile the AWS pane shows, which the plugin's sign-in is checked
    /// as.
    #[serde(default)]
    pane: Option<String>,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AgentProfileSource {
    /// The person's own environment, which Sikemux never overrides.
    Environment,
    Project,
    Default,
    /// The only profile there is, with no `[default]` beside it.
    Automatic,
}

/// The profile agents in a project start on, and why. With no profile, the
/// person's own environment holds keys of its own.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct AgentProfile {
    pub profile: Option<String>,
    pub source: AgentProfileSource,
}

/// What the pane shows: the picks for one project, and what they come to.
#[derive(Serialize)]
pub struct AgentProfilePicks {
    pub(crate) project: Option<String>,
    pub(crate) default: Option<String>,
    pub(crate) effective: Option<AgentProfile>,
}

/// The profile agents started in `project` get, or in any project not
/// picked for when `project` is `None`.
pub(crate) fn agent_profile(ctx: &PluginContext, project: Option<&Path>) -> Option<AgentProfile> {
    let picks = read(ctx.data_dir());
    let project = project.map(canonical);
    effective(&picks, project.as_deref(), shell_var, auth::profile_names)
}

/// AWS_PROFILE for an agent or terminal started in `project`.
pub(crate) fn environment(ctx: &PluginContext, project: &Path) -> BTreeMap<String, String> {
    agent_profile(ctx, Some(project))
        .and_then(|picked| picked.profile)
        .map(|profile| BTreeMap::from([("AWS_PROFILE".to_string(), profile)]))
        .unwrap_or_default()
}

pub(crate) fn picks(ctx: &PluginContext, project: Option<&Path>) -> AgentProfilePicks {
    let saved = read(ctx.data_dir());
    let project = project.map(canonical);
    AgentProfilePicks {
        effective: effective(&saved, project.as_deref(), shell_var, auth::profile_names),
        project: project.and_then(|project| saved.projects.get(&project).cloned()),
        default: saved.default,
    }
}

/// Picks `profile` for agents in `project`, or for every project not picked
/// for when `project` is `None`. No profile clears the pick.
pub(crate) fn pick(
    ctx: &PluginContext,
    project: Option<&Path>,
    profile: Option<String>,
) -> AwsResult<AgentProfilePicks> {
    let profile = profile.filter(|profile| !profile.trim().is_empty());
    {
        let _turn = WRITING.locked();
        let mut saved = read(ctx.data_dir());
        match (project.map(canonical), profile) {
            (Some(project), Some(profile)) => {
                saved.projects.insert(project, profile);
            }
            (Some(project), None) => {
                saved.projects.remove(&project);
            }
            (None, profile) => saved.default = profile,
        }
        save(ctx, &saved)?;
    }
    Ok(picks(ctx, project))
}

pub(crate) fn pane_profile(ctx: &PluginContext) -> Option<String> {
    read(ctx.data_dir()).pane
}

/// Picks the profile the AWS pane shows; no profile clears the pick.
pub(crate) fn pick_pane(ctx: &PluginContext, profile: Option<String>) -> AwsResult<Option<String>> {
    let profile = profile.filter(|profile| !profile.trim().is_empty());
    let _turn = WRITING.locked();
    let mut saved = read(ctx.data_dir());
    saved.pane.clone_from(&profile);
    save(ctx, &saved)?;
    Ok(profile)
}

fn save(ctx: &PluginContext, picks: &Picks) -> AwsResult<()> {
    let bytes = serde_json::to_vec_pretty(picks)?;
    sikemux_process::files::write_atomic(&ctx.data_dir().join(PICKS_FILE), &bytes, false)?;
    Ok(())
}

fn effective(
    picks: &Picks,
    project: Option<&Path>,
    own: impl Fn(&str) -> Option<String>,
    profile_names: impl FnOnce() -> Vec<String>,
) -> Option<AgentProfile> {
    let chosen = |profile: &String, source| {
        Some(AgentProfile {
            profile: Some(profile.clone()),
            source,
        })
    };
    if OWN_IDENTITY.iter().any(|name| own(name).is_some()) {
        return Some(AgentProfile {
            profile: own("AWS_PROFILE").or_else(|| own("AWS_DEFAULT_PROFILE")),
            source: AgentProfileSource::Environment,
        });
    }
    if let Some(profile) = project.and_then(|project| picks.projects.get(project)) {
        return chosen(profile, AgentProfileSource::Project);
    }
    if let Some(profile) = &picks.default {
        return chosen(profile, AgentProfileSource::Default);
    }
    match profile_names().as_slice() {
        [only] if only != "default" => chosen(only, AgentProfileSource::Automatic),
        _ => None,
    }
}

fn read(data_dir: &Path) -> Picks {
    std::fs::read(data_dir.join(PICKS_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Projects are keyed by their real folder, as the core names them.
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}
