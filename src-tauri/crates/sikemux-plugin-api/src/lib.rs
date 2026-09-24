//! The contract between Sikemux and a plugin. A plugin sees only what is in
//! this crate: every exchange with the host is a method name and JSON, so the
//! same plugin can later run in its own process without changing.

mod error;
mod manifest;

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use serde::de::DeserializeOwned;
use serde_json::Value;

pub use error::{PluginError, PluginResult};
pub use manifest::{is_valid_id, AgentTool, Manifest};

pub type PluginFuture<'a, T> = Pin<Box<dyn Future<Output = PluginResult<T>> + Send + 'a>>;

pub trait Plugin: Send + Sync {
    fn manifest(&self) -> &Manifest;

    fn call<'a>(
        &'a self,
        ctx: &'a PluginContext,
        method: &'a str,
        params: Value,
    ) -> PluginFuture<'a, Value>;

    /// Runs until the stream is finished. The host stops a stream by dropping
    /// this future, so a plugin never needs its own cancellation bookkeeping.
    fn stream<'a>(
        &'a self,
        _ctx: &'a PluginContext,
        method: &'a str,
        _params: Value,
        _sink: StreamSink,
    ) -> PluginFuture<'a, ()> {
        Box::pin(async move { Err(PluginError::unknown_method(method)) })
    }

    fn diagnostics(&self) -> Value {
        Value::Null
    }
}

pub struct PluginContext {
    data_dir: PathBuf,
}

impl PluginContext {
    pub fn new(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }

    /// A directory only this plugin writes to. It may not exist yet.
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }
}

type Deliver = Box<dyn Fn(Value) -> bool + Send + Sync>;

pub struct StreamSink(Deliver);

impl StreamSink {
    pub fn new(deliver: impl Fn(Value) -> bool + Send + Sync + 'static) -> Self {
        Self(Box::new(deliver))
    }

    pub fn send(&self, item: Value) -> PluginResult<()> {
        if (self.0)(item) {
            Ok(())
        } else {
            Err(PluginError::stream_closed())
        }
    }
}

pub fn params<T: DeserializeOwned>(params: Value) -> PluginResult<T> {
    Ok(serde_json::from_value(params)?)
}

pub fn reply<T: serde::Serialize>(value: T) -> PluginResult<Value> {
    serde_json::to_value(value).map_err(|error| PluginError::new("internal", error.to_string()))
}
