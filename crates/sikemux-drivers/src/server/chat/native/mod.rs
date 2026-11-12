pub mod acp;
pub mod claude;
pub mod codex;
mod commands;
mod json;
mod models;
pub mod muse;
mod parts;
mod patch;
mod pending;
pub mod pi;
pub mod process;
mod rpc;
mod rpc_driver;
mod servers;
pub mod session;
mod turns;
mod words;

use sikemux_fold::update::stamp_replayed;

/// How an agent is shown a file the person attached: a markdown link named
/// for the file, the way the agents' own clients write it.
pub fn file_link(name: Option<&str>, uri: &str) -> String {
    match name.filter(|name| !name.is_empty()) {
        Some(name) => format!("[@{name}]({uri})"),
        None => match uri.strip_prefix("file://") {
            Some(path) => format!("[@{}]({uri})", path.rsplit('/').next().unwrap_or(path)),
            None => uri.to_owned(),
        },
    }
}
