//! `sikemux chat` run outside every agent: it asks the person to let it in once, then calls the
//! chat tools with the token it was given, over the tool endpoint.

use crate::server::parts::Parts;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use super::{changed, chats, Answer, Via};
use crate::cli::protocol::HarnessRequest;
use crate::server::caller::text_param;
use crate::server::Core;

/// Shorter than the CLI waits for any answer, so it asks again until the person answers.
const WAIT_STEP: Duration = Duration::from_secs(50);

fn outside_only(request: &HarnessRequest) -> Result<(), String> {
    match request.agent_id {
        Some(_) => Err("An agent Sikemux started reaches chats with its agent_* tools; `sikemux chat` in its terminal uses them by itself".into()),
        None => Ok(()),
    }
}

pub(crate) async fn sign_in(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    outside_only(request)?;
    let name = text_param(&request.params, "name", super::NAME_LIMIT)?.unwrap_or("A terminal");
    let id = core
        .outside()
        .ask(name, Via::Cli, None)
        .map_err(|error| error.to_string())?;
    changed(core);
    let check = core
        .outside()
        .waiting_shown(&id)
        .map(|shown| shown.check)
        .unwrap_or_default();
    Ok(json!({ "id": id, "check": check }))
}

pub(crate) async fn sign_in_wait(
    core: &Arc<Core>,
    request: &HarnessRequest,
) -> Result<Value, String> {
    outside_only(request)?;
    let id = text_param(&request.params, "id", 64)?.ok_or("id is required")?;
    match core.outside().wait_for_answer(id, WAIT_STEP).await {
        Answer::Waiting => Ok(json!({ "waiting": true })),
        Answer::Allowed(_) => {
            let (token, client) = core
                .outside()
                .admit(id)
                .map_err(|error| error.to_string())?;
            changed(core);
            Ok(json!({ "token": token, "access": client.access, "expiresAt": client.expires_at }))
        }
        Answer::Declined => Err("The person declined this sign-in in Sikemux".into()),
        Answer::TimedOut => {
            Err("Nobody answered in Sikemux within two minutes; run the command again".into())
        }
    }
}

pub(crate) async fn call(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    outside_only(request)?;
    let token = text_param(&request.params, "token", 256)?.ok_or("token is required")?;
    let caller = core
        .outside()
        .caller(token)
        .ok_or("This terminal's sign-in has expired or was removed in Sikemux; sign in again")?;
    let tool = text_param(&request.params, "tool", 64)?.ok_or("tool is required")?;
    let args = request
        .params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    chats::call(core, &caller, tool, &args).await
}
