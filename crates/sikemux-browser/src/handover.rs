//! Agents keep working in the core while the app quits or restarts. Each
//! agent's tab strip is saved by the old app and comes back unloaded in the
//! new one, so a tab loads its address again once it is shown or used.

use std::path::PathBuf;
use std::sync::atomic::Ordering;

use serde::{Deserialize, Serialize};
use sikemux_process::sync::Locked;

use crate::manager::{validate_agent_id, AgentBrowser, BrowserManager, TabPage};
use crate::viewport::Viewport;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedStrip {
    agent_id: String,
    /// The linked host the agent runs on; none for this one.
    host: Option<String>,
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
    #[serde(default)]
    all_widths: bool,
    /// A file shown over a loopback address that dies with the app, shared again under a new one.
    local_file: Option<PathBuf>,
    /// The person's tab, which tidying up after the agent leaves open.
    #[serde(default)]
    yours: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    profile_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    profile_store_uuid: Option<String>,
    #[serde(default)]
    profile_override: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    profile_override_by: Option<String>,
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
    pub fn saved_strips(&self, host_of: impl Fn(&str) -> Option<String>) -> Vec<SavedStrip> {
        self.agents
            .locked()
            .iter()
            .filter(|(_, agent)| !agent.strip.order.is_empty())
            .map(|(agent_id, agent)| SavedStrip {
                agent_id: agent_id.clone(),
                host: host_of(agent_id),
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
                            all_widths: agent.all_widths.contains(id),
                            local_file: self.local_files.path_of(&page.url),
                            yours: agent.strip.yours(id),
                            profile_id: page.profile_id.clone(),
                            profile_store_uuid: page.profile_store.uuid().map(|u| u.to_string()),
                            profile_override: page.profile_override,
                            profile_override_by: page.profile_override_by.clone(),
                        })
                    })
                    .collect(),
                active: agent.strip.active.clone(),
            })
            .collect()
    }

    /// Puts the saved strips back with every tab unloaded, and names the agents they went to
    /// with the host each runs on.
    pub fn reopen(&self, strips: Vec<SavedStrip>) -> Vec<(String, Option<String>)> {
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
                    Some(path) => {
                        let reopened =
                            match sikemux_base::pages::page_id_of(path).zip(path.parent()) {
                                Some((id, dir)) => {
                                    let resolved = if path.is_file() {
                                        path.clone()
                                    } else {
                                        sikemux_base::pages::page_file(dir, id)
                                            .unwrap_or_else(|| path.clone())
                                    };
                                    let media = Some(sikemux_base::pages::media_folder(dir, id))
                                        .filter(|media| media.is_dir());
                                    let written_in = sikemux_base::pages::written_in(dir, id);
                                    self.themed_page_url(
                                        &resolved,
                                        media.as_deref(),
                                        written_in.as_deref(),
                                    )
                                }
                                None => self.local_files.url_for(path),
                            };
                        match reopened {
                            Ok(url) => url,
                            Err(_) => continue,
                        }
                    }
                    None => tab.url,
                };
                if let Some(number) = tab_number(&tab.id) {
                    self.next_tab.fetch_max(number + 1, Ordering::AcqRel);
                }
                if let Some(fixed) = tab.viewport.as_ref().and_then(SavedViewport::viewport) {
                    agent.viewports.insert(tab.id.clone(), fixed);
                }
                let profile_store = tab
                    .profile_store_uuid
                    .as_deref()
                    .and_then(|raw| uuid::Uuid::parse_str(raw).ok())
                    .map(crate::ProfileStore::Named)
                    .unwrap_or(crate::ProfileStore::Default);
                agent.strip.insert(
                    tab.id.clone(),
                    TabPage {
                        title: tab.title,
                        url,
                        favicon: tab.favicon,
                        profile_id: tab.profile_id,
                        profile_store,
                        profile_override: tab.profile_override,
                        profile_override_by: tab.profile_override_by,
                        ..TabPage::default()
                    },
                );
                if tab.all_widths {
                    agent.all_widths.insert(tab.id.clone());
                }
                if tab.yours {
                    agent.strip.kept.insert(tab.id.clone());
                }
                agent.strip.unloaded.insert(tab.id);
            }
            if agent.strip.order.is_empty() {
                continue;
            }
            if let Some(active) = saved.active {
                agent.strip.activate(&active);
            }
            reopened.push((saved.agent_id.clone(), saved.host));
            agents.insert(saved.agent_id, agent);
        }
        reopened
    }
}
