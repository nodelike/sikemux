use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
pub struct PageParams {
    pub id: String,
}

#[derive(Serialize, Deserialize)]
pub struct PageHtml {
    pub html: String,
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
