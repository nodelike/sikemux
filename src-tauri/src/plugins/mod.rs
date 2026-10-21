//! The plugins run in the core, so the window has none of its own: these
//! commands take the panes' calls to the core, and their streams over its
//! tool endpoint.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use sikemux_core::cli::protocol::{CliServerResponse, MAX_CLI_RESPONSE_BYTES};
use sikemux_core::server::plugins::{Failure, Outcome, StreamEvent};
use sikemux_plugin_api::PluginError;
use sikemux_process::sync::Locked;
use tauri::ipc::Channel;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::task::AbortHandle;

use crate::error::{AppError, AppResult};
use crate::pty::{core_error, PtyManager};

/// The streams the page has open, each a connection to the core that ends
/// the stream there when it closes.
#[derive(Default)]
pub struct PluginStreams {
    next: AtomicU32,
    open: Arc<Mutex<HashMap<u32, AbortHandle>>>,
}

impl PluginStreams {
    pub fn count(&self) -> usize {
        self.open.locked().len()
    }

    pub fn stop(&self, stream_id: u32) {
        let stream = self.open.locked().remove(&stream_id);
        if let Some(stream) = stream {
            stream.abort();
        }
    }

    pub fn drain(&self) {
        let streams: Vec<AbortHandle> = self
            .open
            .locked()
            .drain()
            .map(|(_, stream)| stream)
            .collect();
        for stream in streams {
            stream.abort();
        }
    }
}

fn plugin_error(failure: Failure) -> AppError {
    let error = PluginError::new(failure.category, failure.message);
    AppError::Plugin {
        plugin: failure.plugin,
        error: match failure.status {
            Some(status) => error.with_status(status),
            None => error,
        },
    }
}

async fn forward(manager: &PtyManager, method: &str, params: Value) -> AppResult<Value> {
    let client = manager.client().await?;
    client.call(method, params).await.map_err(core_error)
}

#[tauri::command]
pub async fn plugin_call(
    manager: tauri::State<'_, PtyManager>,
    plugin: String,
    method: String,
    params: Value,
) -> AppResult<Value> {
    let answer = forward(
        &manager,
        "plugin",
        json!({ "plugin": plugin, "method": method, "params": params }),
    )
    .await?;
    match serde_json::from_value(answer)? {
        Outcome::Value(value) => Ok(value),
        Outcome::Failed(failure) => Err(plugin_error(failure)),
    }
}

#[tauri::command]
pub async fn plugin_stream_start(
    manager: tauri::State<'_, PtyManager>,
    streams: tauri::State<'_, PluginStreams>,
    plugin: String,
    method: String,
    params: Value,
    on_event: Channel<StreamEvent>,
) -> AppResult<u32> {
    let answer = forward(
        &manager,
        "pluginStream",
        json!({ "plugin": plugin, "method": method, "params": params }),
    )
    .await?;
    let ticket = answer["ticket"]
        .as_str()
        .ok_or_else(|| AppError::Other("the core gave no ticket for the stream".into()))?
        .to_owned();
    let endpoint: PathBuf = crate::cli_paths::cli_endpoint_path()
        .ok_or_else(|| AppError::Other("Sikemux's tool endpoint is unavailable".into()))?;
    let connection = tauri::async_runtime::spawn_blocking(move || {
        crate::cli_client::open_plugin_stream(&endpoint, ticket)
    })
    .await
    .map_err(|error| AppError::Other(error.to_string()))?
    .map_err(AppError::Other)?;
    connection.set_nonblocking(true)?;
    let connection = tokio::net::TcpStream::from_std(connection)?;

    let stream_id = streams.next.fetch_add(1, Ordering::Relaxed);
    let open = Arc::clone(&streams.open);
    let (registered, is_registered) = tokio::sync::oneshot::channel::<()>();
    let task = tauri::async_runtime::spawn(async move {
        if is_registered.await.is_ok() {
            let last = relay(connection, &on_event, &plugin).await;
            let _ = on_event.send(last);
        }
        open.locked().remove(&stream_id);
    });
    streams
        .open
        .locked()
        .insert(stream_id, task.inner().abort_handle());
    let _ = registered.send(());
    Ok(stream_id)
}

/// Hands the page each item the core writes, and returns how the stream ended.
async fn relay(
    connection: tokio::net::TcpStream,
    on_event: &Channel<StreamEvent>,
    plugin: &str,
) -> StreamEvent {
    let mut lines = BufReader::new(connection);
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = (&mut lines)
            .take(MAX_CLI_RESPONSE_BYTES + 1)
            .read_until(b'\n', &mut line)
            .await;
        let event = match read {
            Ok(0) | Err(_) => None,
            Ok(_) => match serde_json::from_slice::<CliServerResponse>(&line) {
                Ok(CliServerResponse::Result { value }) => serde_json::from_value(value).ok(),
                Ok(CliServerResponse::Error { message }) => Some(StreamEvent::Error {
                    error: Failure::new(plugin, "core", message),
                }),
                _ => None,
            },
        };
        match event {
            Some(StreamEvent::Item { value }) => {
                if on_event.send(StreamEvent::Item { value }).is_err() {
                    return StreamEvent::End;
                }
            }
            Some(last) => return last,
            None => {
                return StreamEvent::Error {
                    error: Failure::new(
                        plugin,
                        "core",
                        "Sikemux's background process ended the stream",
                    ),
                }
            }
        }
    }
}

#[tauri::command]
pub async fn plugin_stream_stop(
    streams: tauri::State<'_, PluginStreams>,
    stream_id: u32,
) -> AppResult<()> {
    streams.stop(stream_id);
    Ok(())
}
