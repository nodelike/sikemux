//! What the Vercel plugin hands its pane and agents: projects, deployments, a build log, a
//! project's domains and the names of its environment variables.

use serde::{Deserialize, Serialize};

/// A production deployment: where it answers and how it stands. `state` is Vercel's own:
/// BUILDING, ERROR, INITIALIZING, QUEUED, READY or CANCELED.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Production {
    pub id: String,
    pub url: Option<String>,
    pub state: String,
    pub created_at: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub framework: Option<String>,
    pub updated_at: u64,
    pub repo: Option<String>,
    pub git_provider: Option<String>,
    pub production_branch: Option<String>,
    /// The deployment serving production.
    pub production: Option<Production>,
    /// A newer production build that is still building or failed, while the one before it serves.
    pub latest_production: Option<Production>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Target {
    Production,
    Preview,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Deployment {
    pub id: String,
    pub project: String,
    pub project_id: Option<String>,
    pub url: Option<String>,
    pub state: String,
    pub target: Target,
    pub created_at: u64,
    pub ready_at: Option<u64>,
    /// How long the build ran, once it finished.
    pub build_ms: Option<u64>,
    pub branch: Option<String>,
    pub commit: Option<String>,
    pub message: Option<String>,
    pub author: Option<String>,
    /// The deployment this one built again, for a redeploy.
    pub redeploy_of: Option<String>,
    pub inspector_url: Option<String>,
    pub aliases: Option<Vec<String>>,
    pub error: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    pub at: u64,
    /// stdout, stderr, command or fatal.
    pub kind: String,
    pub text: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Log {
    pub deployment: Deployment,
    pub lines: Vec<LogLine>,
    /// Lines left out before the ones here.
    pub skipped: usize,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Domain {
    pub name: String,
    pub verified: bool,
    pub redirect: Option<String>,
    pub git_branch: Option<String>,
    pub created_at: Option<u64>,
}

/// An environment variable without its value, which never leaves Vercel through Sikemux.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Variable {
    pub id: String,
    pub key: String,
    pub target: Vec<String>,
    #[serde(rename = "type")]
    pub kind: String,
    pub git_branch: Option<String>,
    pub updated_at: Option<u64>,
}

/// The projects of the team the pane shows.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Projects {
    pub projects: Vec<Project>,
}

/// A project's latest deployments, newest first.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Deployments {
    pub deployments: Vec<Deployment>,
}

/// A deployment with the end of its build log, and the text an agent is handed of both.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OpenLog {
    pub log: Log,
    pub for_agent: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Domains {
    pub domains: Vec<Domain>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Variables {
    pub variables: Vec<Variable>,
}

/// The Vercel account signed in.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    /// Vercel's user id.
    pub id: String,
    pub username: String,
    pub name: String,
    pub email: String,
    #[serde(default)]
    pub sign_in: SignIn,
}

#[derive(Serialize, Deserialize, Clone, Copy, Default, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum SignIn {
    #[default]
    Token,
    /// The Sikemux integration, installed from the browser on one team at a time.
    Browser,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Team {
    pub id: String,
    pub slug: String,
    pub name: String,
}

/// Who is signed in and the teams it can see, as the plugin's `status` says.
#[derive(Serialize, Deserialize, Clone, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Signed {
    pub account: Option<Account>,
    pub teams: Vec<Team>,
    /// The team calls use when they name none, by id; none for the account's own projects.
    pub team: Option<String>,
    /// Whether the account has projects of its own beside its teams. Newer accounts
    /// keep theirs in a default team instead, which is then the team chosen.
    pub personal: bool,
    /// Whether this build can sign in through the browser.
    pub browser_sign_in: bool,
}
