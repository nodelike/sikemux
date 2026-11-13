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
