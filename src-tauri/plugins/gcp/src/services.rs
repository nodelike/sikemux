// Service Usage: turning on a Google API a page needs, waiting until Google
// reports it on.

use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::Target;
use crate::client::{self, at, segment};
use crate::error::{GcpError, GcpResult};

const API: &str = "https://serviceusage.googleapis.com/v1";
const WAIT: Duration = Duration::from_secs(2);
const MAX_WAITS: usize = 45;

#[derive(Deserialize)]
pub struct EnableInput {
    #[serde(flatten)]
    pub target: Target,
    pub service: String,
}

/// Turns on an API in the project and returns once it is on.
pub async fn enable(input: EnableInput) -> GcpResult<Value> {
    let project = input.target.project()?;
    let service = segment("service", &input.service)?;
    let url = format!("{API}/projects/{project}/services/{service}:enable");
    let mut operation = client::post(&input.target, &url, &json!({})).await?;
    for _ in 0..MAX_WAITS {
        if at(&operation, "/done").as_bool() == Some(true) {
            break;
        }
        let Some(name) = at(&operation, "/name").as_str().map(str::to_string) else {
            break;
        };
        tokio::time::sleep(WAIT).await;
        operation = client::get(&input.target, &format!("{API}/{name}"), &[]).await?;
    }
    if let Some(message) = at(&operation, "/error/message").as_str() {
        return Err(GcpError::Response(message.to_string()));
    }
    Ok(
        json!({ "service": input.service, "enabled": at(&operation, "/done").as_bool() == Some(true) }),
    )
}
