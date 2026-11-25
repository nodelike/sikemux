use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
pub struct PageParams {
    pub id: String,
    /// The agent whose desk it would open on, such as the one whose ask links it; the one that
    /// showed it when none is named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
}

impl PageParams {
    pub fn new(id: &str) -> Self {
        Self {
            id: id.to_owned(),
            agent: None,
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct PageHtml {
    pub html: String,
    /// Where the core keeps the page on its computer, so a local app can serve it directly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<std::path::PathBuf>,
    /// A JPEG of the page as its desk tab first drew it, in base64, for a page shown in a tab.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub picture: Option<String>,
    /// The folder on the core's computer holding the videos and sounds the page names under it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media: Option<std::path::PathBuf>,
    /// The folder on the core's computer the agent wrote the page in, which the page's other
    /// relative addresses are served from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub written_in: Option<std::path::PathBuf>,
    /// The open agent whose desk it opens on, or none when nobody it belongs with is open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desk: Option<String>,
}

/// A project's pages, narrowed to those whose title holds every word in `words`.
#[derive(Deserialize, Serialize, Default)]
pub struct PagesParams {
    /// The project's folder.
    pub project: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub words: String,
}

/// The latest version of a page an agent showed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListedPage {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub project: String,
    /// The name of the agent that showed it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub agent: String,
    pub shown_at: u64,
    pub versions: u64,
    #[serde(flatten)]
    pub life: PageLife,
    /// The link it is shared through, while it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared: Option<PageShare>,
}

/// A page's link, which shows its latest version until it ends or sharing stops.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageShare {
    pub url: String,
    pub expires_at: u64,
    /// Whether anyone with the link may read it, or only people signed in to Sikemux.
    pub anyone: bool,
    /// Why the latest version could not update the link, once the core stopped retrying.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

/// How long Sikemux keeps a page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "life", rename_all = "camelCase")]
pub enum PageLife {
    Kept,
    /// It goes at `at`, in milliseconds since the epoch, unless it is kept first.
    Goes {
        at: u64,
    },
    /// It stays while the call it went with is open, and goes a day after that call settles.
    WithCall,
}

/// Shares page `id` through a link. A page already shared keeps its link and when it ends.
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SharePageParams {
    pub id: String,
    pub lifetime_ms: u64,
    pub anyone: bool,
    /// The person was told that our server and chat apps can read the page's title and their name.
    #[serde(default)]
    pub told: bool,
    /// The `:root` rule of the theme the page is read in.
    pub theme: String,
}

#[derive(Deserialize, Serialize)]
pub struct KeepPageParams {
    pub id: String,
    pub keep: bool,
}

/// The runs that match every field given.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSelector {
    pub execution_id: Option<String>,
    pub project: Option<String>,
    pub agent_id: Option<String>,
}

impl RunSelector {
    /// The one run `execution_id` names.
    pub fn of_execution(execution_id: &str) -> Self {
        Self {
            execution_id: Some(execution_id.to_owned()),
            project: None,
            agent_id: None,
        }
    }

    pub fn of_agent(agent_id: &str) -> Self {
        Self {
            execution_id: None,
            project: None,
            agent_id: Some(agent_id.to_owned()),
        }
    }
}
