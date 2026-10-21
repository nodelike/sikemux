//! An update restarts the app while agents keep working in the core. Each
//! agent's tab strip is saved as the old app quits and comes back unloaded in
//! the new one, so a tab loads its address again once it is shown or used.

use std::path::PathBuf;
use std::sync::atomic::Ordering;

use serde::{Deserialize, Serialize};
use sikemux_process::sync::Locked;
use tauri::AppHandle;

use super::{validate_agent_id, viewport::Viewport, AgentBrowser, BrowserManager, TabPage};

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedStrip {
    agent_id: String,
    tabs: Vec<SavedTab>,
    active: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedTab {
    id: String,
    url: String,
    title: String,
    favicon: Option<String>,
    viewport: Option<SavedViewport>,
    /// A file shown over a loopback address that dies with the app, shared again under a new one.
    local_file: Option<PathBuf>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedViewport {
    width: u32,
    height: u32,
    preset: Option<String>,
}

impl SavedViewport {
    fn viewport(&self) -> Option<Viewport> {
        match &self.preset {
            Some(name) => Viewport::preset(name),
            None => Viewport::sized(self.width, self.height).ok(),
        }
    }
}

/// The counter a tab's id ends in, which new tabs must not reuse.
fn tab_number(tab_id: &str) -> Option<u64> {
    tab_id.rsplit('-').next()?.parse().ok()
}

fn plain_tab_id(tab_id: &str) -> bool {
    tab_id.starts_with("browser-")
        && tab_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

impl BrowserManager {
    pub fn saved_strips(&self) -> Vec<SavedStrip> {
        self.agents
            .locked()
            .iter()
            .filter(|(_, agent)| !agent.strip.order.is_empty())
            .map(|(agent_id, agent)| SavedStrip {
                agent_id: agent_id.clone(),
                tabs: agent
                    .strip
                    .order
                    .iter()
                    .filter_map(|id| {
                        let page = agent.strip.pages.get(id)?;
                        Some(SavedTab {
                            id: id.clone(),
                            url: page.url.clone(),
                            title: page.title.clone(),
                            favicon: page.favicon.clone(),
                            viewport: agent.viewports.get(id).map(|fixed| SavedViewport {
                                width: fixed.width,
                                height: fixed.height,
                                preset: fixed.preset.map(str::to_owned),
                            }),
                            local_file: self.local_files.path_of(&page.url),
                        })
                    })
                    .collect(),
                active: agent.strip.active.clone(),
            })
            .collect()
    }

    /// Puts the saved strips back with every tab unloaded, and names the agents they went to.
    pub fn reopen(&self, app: &AppHandle, strips: Vec<SavedStrip>) -> Vec<String> {
        self.adopt_app(app);
        let mut reopened = Vec::new();
        let mut agents = self.agents.locked();
        for saved in strips {
            if validate_agent_id(&saved.agent_id).is_err() || agents.contains_key(&saved.agent_id) {
                continue;
            }
            let mut agent = AgentBrowser::default();
            for tab in saved.tabs {
                if !plain_tab_id(&tab.id) || agent.strip.pages.contains_key(&tab.id) {
                    continue;
                }
                let url = match &tab.local_file {
                    Some(path) => match self.local_files.url_for(path) {
                        Ok(url) => url,
                        Err(_) => continue,
                    },
                    None => tab.url,
                };
                if let Some(number) = tab_number(&tab.id) {
                    self.next_tab.fetch_max(number + 1, Ordering::AcqRel);
                }
                if let Some(fixed) = tab.viewport.as_ref().and_then(SavedViewport::viewport) {
                    agent.viewports.insert(tab.id.clone(), fixed);
                }
                agent.strip.insert(
                    tab.id.clone(),
                    TabPage {
                        title: tab.title,
                        url,
                        favicon: tab.favicon,
                        ..TabPage::default()
                    },
                );
                agent.strip.unloaded.insert(tab.id);
            }
            if agent.strip.order.is_empty() {
                continue;
            }
            if let Some(active) = saved.active {
                agent.strip.activate(&active);
            }
            reopened.push(saved.agent_id.clone());
            agents.insert(saved.agent_id, agent);
        }
        reopened
    }
}
