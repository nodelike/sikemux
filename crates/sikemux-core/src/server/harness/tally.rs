//! How often each agent tool was called, and how often it worked.

use super::*;

/// A plugin tool is named by the agent's tool name, anything else by its
/// method. Listing plugin tools is not a call.
pub(super) fn tool_name(request: &HarnessRequest) -> Option<String> {
    match request.method.as_str() {
        method if !methods::of(method).counted => None,
        "plugins.call" => request
            .params
            .get("tool")
            .and_then(Value::as_str)
            .map(str::to_owned),
        method => Some(method.to_owned()),
    }
}

#[derive(Default, Serialize, Deserialize, PartialEq, Debug)]
pub(super) struct Tally {
    pub(super) calls: u64,
    pub(super) failures: u64,
}

/// How often agents call each tool and how often a call fails, so the tool
/// surface can be trimmed by what is used. The tally stays on this machine.
pub(super) fn record_tool_call(path: &Path, tool: &str, succeeded: bool) {
    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());
    let _held = ONE_AT_A_TIME.locked();
    if let Err(error) = add_tool_call(path, tool, succeeded) {
        eprintln!("sikemux core: could not count a call to {tool}: {error}");
    }
}

pub(super) fn add_tool_call(path: &Path, tool: &str, succeeded: bool) -> std::io::Result<()> {
    let mut tallies: BTreeMap<String, Tally> = std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    let tally = tallies.entry(tool.to_owned()).or_default();
    tally.calls += 1;
    if !succeeded {
        tally.failures += 1;
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, serde_json::to_vec_pretty(&tallies)?)
}
