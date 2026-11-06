//! The page a slice's change shows, which the core opens and takes itself in
//! its owner's browser once the check passed on a change to the project's
//! pages, so evidence of the running thing exists without the agent asking.
//! It opens only a server that is running already, and only the server's own
//! address, never the route the change touched.

use std::path::Path;
use std::sync::Arc;

use serde_json::json;

use super::slice::other_widths;
use super::slice::{decode_shots, keep_shots};
use crate::cli::protocol::HarnessRequest;
use crate::server::harness::forward;
use crate::server::ports::{self, PortOwner};
use crate::server::Core;
use crate::tasks::config::{self, ConfigLoad, Stated};
use crate::tasks::detect;

/// The endings of the files a browser draws a page from.
const PAGE_FILES: [&str; 12] = [
    "tsx", "jsx", "vue", "svelte", "astro", "html", "htm", "css", "scss", "sass", "less", "styl",
];

/// How long the page may take to settle once it loads.
const SETTLE_MS: u64 = 10_000;

pub(super) fn is_page_file(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|ending| ending.to_str())
        .is_some_and(|ending| PAGE_FILES.contains(&ending.to_ascii_lowercase().as_str()))
}

/// Takes the page the slice on `card` shows at `head` in its owner's browser
/// and keeps it as the slice's evidence, when the window is open and the
/// page's address can be worked out.
pub(super) fn take(core: &Core, project: String, card: u64, owner: String, head: String) {
    if !core.window.is_open() {
        return;
    }
    let Some(url) = address(core, &project, &owner) else {
        return;
    };
    let (Some(core), Ok(runtime)) = (core.arc(), tokio::runtime::Handle::try_current()) else {
        return;
    };
    runtime.spawn(async move {
        let shots = shoot(&core, &project, card, &owner, &url).await;
        let decoded = decode_shots(&shots);
        if decoded.is_empty() {
            return;
        }
        tokio::task::spawn_blocking(move || {
            keep_shots(&core, &project, card, &decoded, |slice, kept| {
                slice.page(&owner, &head, &url, kept)
            });
        });
    });
}

/// Where the owner's change can be seen running: a server the owner started
/// itself, which serves its own checkout; else, when the owner works in the
/// project's own checkout, what the project's preview points at, or a server
/// its run task started. None when the project turns this off with
/// `"preview": false`, or nothing is running.
fn address(core: &Core, project: &str, owner: &str) -> Option<String> {
    let preview = match config::load(Path::new(project)) {
        ConfigLoad::Valid { config, .. } => config.preview,
        ConfigLoad::Absent { .. } => None,
        ConfigLoad::Invalid { .. } => return None,
    };
    if matches!(preview, Some(Stated::Off)) {
        return None;
    }
    let listening = ports::listening_now(core);
    if let Some(own) = listening
        .iter()
        .find(|listening| listening.owner.started_by(owner))
    {
        return Some(format!("http://localhost:{}/", own.port));
    }
    let own_checkout = core
        .workspaces
        .agent(owner)
        .is_some_and(|agent| agent.worktree.is_some());
    if own_checkout {
        return None;
    }
    let ports = ports::for_project(listening, Path::new(project));
    ports
        .iter()
        .find(|port| port.preview)
        .or_else(|| {
            ports.iter().find(|port| {
                matches!(&port.listening.owner, PortOwner::Task { task_id, .. } if detect::is_run_task(task_id))
            })
        })
        .map(|port| port.url.clone())
}

/// Opens `url` in a tab of the owner's and takes it at its own width and at
/// each of the widths a slice's screenshots are also taken at, then closes the
/// tab again when it was a new one. A tab of the owner's already on that site
/// is used instead of a new one, and left open.
async fn shoot(
    core: &Arc<Core>,
    project: &str,
    card: u64,
    owner: &str,
    url: &str,
) -> Vec<serde_json::Value> {
    let asked = |method: &str, params: serde_json::Value, step: &str| HarnessRequest {
        id: format!("slice-page-{card}-{step}"),
        project: project.to_owned(),
        agent_id: Some(owner.to_owned()),
        method: method.to_owned(),
        params,
    };
    let had = forward(core, asked("browser.state", json!({}), "tabs"))
        .await
        .map(|state| tab_ids(&state))
        .unwrap_or_default();
    let opened = asked(
        "browser.navigate",
        json!({ "url": url, "newTab": true, "waitFor": { "networkIdle": true, "timeoutMs": SETTLE_MS } }),
        "open",
    );
    let tab = match forward(core, opened).await {
        Ok(opened) => opened["tabId"].as_str().map(str::to_owned),
        Err(error) => {
            eprintln!("sikemux core: the slice's page at {url} did not open: {error}");
            return Vec::new();
        }
    };
    let shot = asked("browser.screenshot", json!({}), "shot");
    let mut shots = Vec::new();
    match forward(core, shot.clone()).await {
        Ok(first) => {
            shots.push(first);
            shots.extend(other_widths(core, &shot, owner).await);
        }
        Err(error) => eprintln!("sikemux core: the slice's page at {url} was not taken: {error}"),
    }
    if let Some(tab) = tab.filter(|tab| !had.contains(tab)) {
        let close = asked("browser.tab.close", json!({ "tabId": tab }), "close");
        if let Err(error) = forward(core, close).await {
            eprintln!("sikemux core: the tab the slice's page opened in was not closed: {error}");
        }
    }
    shots
}

fn tab_ids(state: &serde_json::Value) -> Vec<String> {
    state["tabs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|tab| tab["tabId"].as_str().map(str::to_owned))
        .collect()
}
