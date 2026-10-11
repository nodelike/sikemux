//! The core's methods a client reaches by name through `Request::Call`. A
//! device gains a method here without its native code changing, so one a
//! released app calls is never removed or renamed.

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::access::Needs;
use super::attachments;
use super::connection::blocking;
use super::{Core, CoreError, CoreResult};

pub(crate) fn needs(method: &str) -> Needs {
    match method {
        "chatFile" => Needs::Watch,
        _ => Needs::Local,
    }
}

pub(crate) async fn call(core: Arc<Core>, method: &str, params: Value) -> CoreResult<Value> {
    match method {
        "chatFile" => answer(chat_file(&core, parse(params)?).await?),
        _ => Err(format!("this host does not know {method}").into()),
    }
}

fn parse<T: for<'de> Deserialize<'de>>(params: Value) -> CoreResult<T> {
    serde_json::from_value(params)
        .map_err(|error| CoreError::from(format!("the call's params do not fit: {error}")))
}

fn answer(result: impl Serialize) -> CoreResult<Value> {
    Ok(serde_json::to_value(result)?)
}

#[derive(Deserialize)]
struct ChatFileParams {
    path: PathBuf,
}

#[derive(Serialize)]
struct ChatFile {
    data: String,
}

/// A file kept beside the pictures pasted into the app's chats, such as one
/// a message carried, in base64.
async fn chat_file(core: &Core, params: ChatFileParams) -> CoreResult<ChatFile> {
    let dir = core
        .listening
        .get()
        .and_then(|listening| listening.config.attachment_dir.clone())
        .ok_or_else(|| CoreError::from("this host keeps no files for chats"))?;
    let data = blocking(move || attachments::read(&dir, &params.path)).await?;
    Ok(ChatFile { data })
}
