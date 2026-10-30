//! Waiting for a tab's page to lay out at the size its viewport asks for.

use std::time::{Duration, Instant};

use sikemux_process::sync::Locked;

use crate::manager::BrowserManager;
use crate::viewport::{matches, parse_inner_size};

const SETTLE_TIMEOUT: Duration = Duration::from_secs(3);
const SETTLE_POLL: Duration = Duration::from_millis(50);

impl BrowserManager {
    /// The size the tab's page should lay out at now, in CSS pixels.
    pub fn layout_size(&self, agent_id: &str, tab_id: &str) -> Option<(f64, f64)> {
        let agents = self.agents.locked();
        let agent = agents.get(agent_id)?;
        agent.views.get(tab_id)?;
        Some(agent.layout_of(tab_id).page_size())
    }

    /// Waits until the page itself reports the size it should lay out at, and
    /// returns that size. Call it after `set_viewport` and any reload it caused.
    pub async fn settle_viewport(
        &self,
        agent_id: &str,
        tab_id: &str,
    ) -> Result<(u32, u32), String> {
        let started = Instant::now();
        let mut last = None;
        loop {
            let expected = self
                .layout_size(agent_id, tab_id)
                .ok_or("the tab closed while its viewport was changing")?;
            let view = self
                .agents
                .locked()
                .get(agent_id)
                .and_then(|agent| agent.views.get(tab_id).cloned())
                .ok_or("the tab closed while its viewport was changing")?;
            if let Ok(raw) = crate::tools::eval(&view, r#"`${innerWidth}x${innerHeight}`"#).await {
                if let Some(reported) = parse_inner_size(&raw) {
                    if matches(reported, expected) {
                        return Ok((reported.0 as u32, reported.1 as u32));
                    }
                    last = Some(reported);
                }
            }
            if started.elapsed() >= SETTLE_TIMEOUT {
                return Err(match last {
                    Some((width, height)) => format!(
                        "the page still lays out at {width}x{height}, not the {}x{} asked for",
                        expected.0, expected.1
                    ),
                    None => format!(
                        "the page did not report its size, so {}x{} could not be confirmed",
                        expected.0, expected.1
                    ),
                });
            }
            tokio::time::sleep(SETTLE_POLL).await;
        }
    }
}
