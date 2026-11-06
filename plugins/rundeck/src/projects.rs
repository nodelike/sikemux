// The projects the signed-in account can see.

use serde::{Deserialize, Serialize};

use crate::error::RundeckResult;

use crate::client::{get_json, Session};

#[derive(Serialize, Clone, Deserialize)]
pub struct RundeckProject {
    pub name: String,
    pub description: Option<String>,
}

pub async fn projects(session: &Session) -> RundeckResult<Vec<RundeckProject>> {
    let mut out: Vec<RundeckProject> = get_json(session, "/projects", &[]).await?;
    out.sort_by_key(|project| project.name.to_lowercase());
    Ok(out)
}
