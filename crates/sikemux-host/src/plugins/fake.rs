//! Test kits only: plugins that answer from files instead of their services, so a kit's core
//! never reaches the person's accounts. `dev/plugins/<plugin>/<method>.json` in the config
//! folder holds a list of cases, `{ "params"?, "value" | "failed" }`, the first whose params all
//! match answering. A stream sends each line of `<method>.jsonl` a moment apart, then ends.
//! A call with no file answers as signed out. Every call is written to
//! `dev/plugins/calls.jsonl`, for a kit to check what a pane sent.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use sikemux_core::server::plugins::{
    Answer, Category, Failure, Gated, Listing, Manifest, Plugins, Sink,
};

pub(super) struct Fake(pub(super) Box<dyn Plugins>);

fn folder() -> Option<PathBuf> {
    Some(sikemux_process::paths::config_dir()?.join("dev/plugins"))
}

/// How long a stream waits between the lines it sends.
const TICK: std::time::Duration = std::time::Duration::from_millis(400);

fn signed_out(plugin: &str) -> Failure {
    Failure::new(
        plugin,
        Category::Unconfigured,
        "this test kit has no answer for that",
    )
}

/// The first case of `method`'s file whose params fit.
fn case(plugin: &str, method: &str, params: &Value) -> Result<Value, Failure> {
    let folder = folder().ok_or_else(|| signed_out(plugin))?;
    note(&folder, plugin, method, params);
    let cases = std::fs::read_to_string(folder.join(plugin).join(format!("{method}.json")))
        .ok()
        .and_then(|text| serde_json::from_str::<Vec<Value>>(&text).ok())
        .unwrap_or_default();
    cases
        .into_iter()
        .find(|case| fits(&case["params"], params))
        .ok_or_else(|| signed_out(plugin))
}

fn answer(plugin: &str, method: &str, params: &Value) -> Result<Value, Failure> {
    let case = case(plugin, method, params)?;
    match case.get("failed") {
        Some(failed) => Err(Failure {
            plugin: plugin.to_owned(),
            category: serde_json::from_value(failed["category"].clone()).unwrap_or(Category::Http),
            message: failed["message"].as_str().unwrap_or_default().to_owned(),
            status: failed["status"]
                .as_u64()
                .and_then(|status| status.try_into().ok()),
        }),
        None => Ok(case.get("value").cloned().unwrap_or(Value::Null)),
    }
}

fn fits(wanted: &Value, params: &Value) -> bool {
    match wanted.as_object() {
        Some(wanted) => wanted
            .iter()
            .all(|(key, value)| params.get(key) == Some(value)),
        None => true,
    }
}

fn note(folder: &Path, plugin: &str, method: &str, params: &Value) {
    let line = json!({ "plugin": plugin, "method": method, "params": params });
    let _ = std::fs::create_dir_all(folder).and_then(|()| {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(folder.join("calls.jsonl"))?;
        writeln!(file, "{line}")
    });
}

impl Plugins for Fake {
    fn manifests(&self) -> Vec<&Manifest> {
        self.0.manifests()
    }

    fn set_disabled(&self, ids: Vec<String>) {
        self.0.set_disabled(ids);
    }

    fn disabled(&self) -> Vec<String> {
        self.0.disabled()
    }

    fn relay_sign_in(&self, _: &str) -> Result<(), String> {
        Err("this test kit signs in to nothing".into())
    }

    fn offers_agent_tools(&self, _: &str) -> bool {
        false
    }

    fn agent_environment(&self, _: &Path) -> BTreeMap<String, String> {
        BTreeMap::new()
    }

    fn call<'a>(&'a self, plugin: &'a str, method: &'a str, params: Value) -> Answer<'a, Value> {
        Box::pin(async move { answer(plugin, method, &params) })
    }

    fn stream<'a>(
        &'a self,
        plugin: &'a str,
        method: &'a str,
        params: Value,
        sink: Sink,
    ) -> Answer<'a, ()> {
        Box::pin(async move {
            let folder = folder().ok_or_else(|| signed_out(plugin))?;
            note(&folder, plugin, method, &params);
            let lines =
                std::fs::read_to_string(folder.join(plugin).join(format!("{method}.jsonl")))
                    .map_err(|_| signed_out(plugin))?;
            for item in lines
                .lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            {
                if !sink(item) {
                    break;
                }
                tokio::time::sleep(TICK).await;
            }
            Ok(())
        })
    }

    fn agent_tools<'a>(&'a self, _: &'a [String]) -> Listing<'a> {
        Box::pin(async { Vec::new() })
    }

    fn gated(&self, _: &str) -> Option<Gated> {
        None
    }

    fn call_agent_tool<'a>(&'a self, name: &'a str, _: Value) -> Answer<'a, Value> {
        Box::pin(async move { Err(signed_out(name)) })
    }
}
