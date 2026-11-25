//! A repeating trigger's check: a command run each time the trigger comes due, which fires it
//! only when what the command says has changed since the last time, so a look that finds
//! nothing new posts nothing in the chat. A check that cannot run fires every time, so it
//! never goes quiet while broken.

use crate::server::parts::Parts;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sikemux_process::sync::Locked;

use super::{fire, Firing};
use crate::git::{all_said, tail};
use crate::server::Core;

const CHECK_LIMIT: Duration = Duration::from_secs(30);
const CHECK_OUTPUT: usize = 256 * 1024;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Check {
    command: String,
    dir: String,
    /// The end of what it said the last time it ran.
    said: String,
    /// Whether it has fired and its agent no longer waits on it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    fired: bool,
    /// Whether the turn it last prompted has yet to end, which says if its agent still waits.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    prompted: bool,
}

impl Check {
    /// Runs `command` in `dir` once, for what later runs are compared with.
    pub(super) async fn first(command: String, dir: String) -> Result<Self, String> {
        let said = run(&command, &dir).await?;
        Ok(Self {
            command,
            dir,
            said,
            fired: false,
            prompted: false,
        })
    }

    pub(super) fn said(&self) -> &str {
        &self.said
    }

    pub(super) fn fired(&self) -> bool {
        self.fired
    }

    pub(super) fn prompted(&self) -> bool {
        self.prompted
    }

    /// The turn it prompted ended, its agent still waiting on it or not.
    pub(super) fn turn_ended(&mut self, waits: bool) {
        if std::mem::take(&mut self.prompted) {
            self.fired = !waits;
        }
    }
}

async fn run(command: &str, dir: &str) -> Result<String, String> {
    let (command, dir) = (command.to_owned(), dir.to_owned());
    tokio::task::spawn_blocking(move || {
        let mut shell = sikemux_process::user_environment::background_shell()?;
        shell.args(["-c", &command]).current_dir(Path::new(&dir));
        let output = sikemux_process::run(&mut shell, None, CHECK_LIMIT, CHECK_OUTPUT, None)
            .map_err(|error| format!("the check did not run: {error}"))?;
        let mut said = tail(&all_said(&output));
        if !output.status.success() {
            said.push_str(&format!("\n({})", output.status));
        }
        Ok(said)
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Runs the check of trigger `id` and fires it with what the check says, when that changed.
pub(super) async fn look(core: Arc<Core>, id: u64) {
    let check = {
        let state = core.triggers().state.locked();
        let trigger = state.triggers.iter().find(|trigger| trigger.id == id);
        trigger.and_then(|trigger| trigger.check.clone())
    };
    let Some(check) = check else { return };
    let ran = run(&check.command, &check.dir).await;
    if ran.as_deref() == Ok(check.said.as_str()) {
        return;
    }
    let firing = core.triggers().change(|state| {
        let trigger = state.triggers.iter_mut().find(|trigger| trigger.id == id)?;
        let check = trigger.check.as_mut()?;
        check.fired = true;
        check.prompted = true;
        if let Ok(said) = &ran {
            check.said = said.clone();
        }
        let cloned = trigger.clone();
        let in_flight = state.start_firing(&core, &cloned);
        Some((cloned, in_flight))
    });
    let Some((mut trigger, in_flight)) = firing else {
        return;
    };
    let news = match ran {
        Ok(said) => format!("says something new:\n{said}"),
        Err(error) => format!("could not run, and fires each time until it can:\n{error}"),
    };
    trigger.prompt = format!("{}\n\nIts check `{}` {news}", trigger.prompt, check.command);
    fire(
        core,
        Firing {
            trigger,
            late: None,
            last: false,
            request: None,
            in_flight,
        },
    )
    .await;
}
