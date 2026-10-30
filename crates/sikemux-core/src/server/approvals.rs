//! Plugin tools the person approves each call to. The tool's check says what
//! the person is asked, the question goes straight to them, and what they pick
//! decides whether the tool's method runs. The question is kept like any other,
//! but the call's arguments may hold a password, so they live in the Keychain.

use std::collections::HashSet;
use std::fmt;
use std::sync::{mpsc, Arc};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::cli::protocol::{HarnessRequest, CORE_REPLY_TIMEOUT};
use crate::protocol::WindowCall;

use super::plugins::{self, Gated};
use super::questions;
use super::Core;
use sikemux_wire::questions::QuestionChoice;

/// A development build keeps entries of its own, so it never reads or deletes the
/// installed app's.
#[cfg(not(test))]
const SERVICE: &str = if cfg!(debug_assertions) {
    "sikemux-approval-dev"
} else {
    "sikemux-approval"
};
#[cfg(test)]
const SERVICE: &str = "sikemux-approval-test";

/// What a choice on the card does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Does {
    /// Runs the tool's method as the agent called it.
    Approve,
    /// Opens the plugin's pane on the call, to change it before it runs.
    Edit,
    Decline,
}

#[derive(Deserialize)]
struct Offered {
    label: String,
    #[serde(default)]
    detail: Option<String>,
    does: Does,
}

/// What the person is asked, as the tool's check answers it.
#[derive(Deserialize)]
struct Proposal {
    header: String,
    text: String,
    choices: Vec<Offered>,
    /// Where the plugin's pane opens to change the call; needed by an `edit` choice.
    #[serde(default)]
    edit: Option<String>,
}

/// The part of a call that may hold a password: what the agent called it with, and the
/// place its plugin's pane opens to change it.
#[derive(Serialize, Deserialize)]
pub(crate) struct Held {
    arguments: Value,
    edit: Option<String>,
}

/// A call waiting on the person, kept with its question; what it holds is kept apart.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Approval {
    plugin: String,
    method: String,
    /// What each of the question's choices does, in their order.
    does: Vec<Does>,
    #[serde(skip)]
    held: Option<Arc<Held>>,
}

impl fmt::Debug for Approval {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Approval")
            .field("plugin", &self.plugin)
            .field("method", &self.method)
            .finish_non_exhaustive()
    }
}

enum Keep {
    Write(String, String),
    Delete(String),
}

/// Keeps what each waiting call holds in the Keychain under its question's id, in the order
/// the questions change, on a thread of its own so the questions never wait on the Keychain.
#[derive(Default)]
pub(crate) struct Keeper {
    sender: Option<mpsc::Sender<Keep>>,
    kept: HashSet<String>,
}

impl Keeper {
    /// A keeper for a core with somewhere to keep its questions; none keeps nothing.
    pub(crate) fn new(keeps: bool) -> Self {
        if !keeps {
            return Self::default();
        }
        let (sender, keeps) = mpsc::channel::<Keep>();
        let started = std::thread::Builder::new()
            .name("sikemux-approvals".into())
            .spawn(move || {
                for keep in keeps {
                    let done = match &keep {
                        Keep::Write(id, held) => sikemux_keychain::write(SERVICE, id, held),
                        Keep::Delete(id) => sikemux_keychain::delete(SERVICE, id),
                    };
                    if let Err(error) = done {
                        eprintln!("sikemux core: an approval's call could not be kept: {error}");
                    }
                }
            });
        Self {
            sender: started.ok().map(|_| sender),
            kept: HashSet::new(),
        }
    }

    /// Reads back what an earlier run kept for the question `id`, saying whether the call can
    /// still be acted on.
    pub(crate) fn restore(&mut self, id: &str, approval: &mut Approval) -> bool {
        if self.sender.is_none() {
            return false;
        }
        let held = sikemux_keychain::read(SERVICE, id)
            .map_err(|error| error.to_string())
            .and_then(|kept| kept.ok_or_else(|| "nothing was kept".to_owned()))
            .and_then(|kept| {
                serde_json::from_str::<Held>(&kept).map_err(|error| error.to_string())
            });
        match held {
            Ok(held) => {
                approval.held = Some(Arc::new(held));
                self.kept.insert(id.to_owned());
                true
            }
            Err(error) => {
                eprintln!(
                    "sikemux core: dropped approval {id}, whose call could not be read: {error}"
                );
                self.forget(id);
                false
            }
        }
    }

    /// Forgets what was kept for the question `id`, which is gone.
    pub(crate) fn forget(&mut self, id: &str) {
        self.kept.remove(id);
        if let Some(sender) = &self.sender {
            let _ = sender.send(Keep::Delete(id.to_owned()));
        }
    }

    /// Keeps the calls of the questions still waiting, and forgets the rest.
    pub(crate) fn keep<'a>(&mut self, waiting: impl Iterator<Item = (&'a str, &'a Approval)>) {
        let Some(sender) = &self.sender else {
            return;
        };
        let mut now = HashSet::new();
        for (id, approval) in waiting {
            now.insert(id.to_owned());
            let held = approval.held.as_deref();
            if let (false, Some(Ok(held))) =
                (self.kept.contains(id), held.map(serde_json::to_string))
            {
                let _ = sender.send(Keep::Write(id.to_owned(), held));
            }
        }
        for gone in self.kept.difference(&now) {
            let _ = sender.send(Keep::Delete(gone.clone()));
        }
        self.kept = now;
    }
}

/// Checks the call and puts it to the person, answering the agent that it waits on them.
pub(crate) async fn propose(
    core: &Arc<Core>,
    request: &HarnessRequest,
    gated: Gated,
    arguments: Value,
) -> Result<Value, String> {
    let agent_id = request
        .agent_id
        .clone()
        .ok_or("this tool needs the calling agent's id")?;
    let checked = plugins::call(core, &gated.plugin, &gated.check, arguments.clone())
        .await
        .map_err(|failure| plugins::explain(&failure))?;
    let proposal: Proposal = serde_json::from_value(checked).map_err(|error| {
        format!(
            "{} answered a check Sikemux cannot read: {error}",
            gated.plugin
        )
    })?;
    let does: Vec<Does> = proposal
        .choices
        .iter()
        .map(|offered| offered.does)
        .collect();
    if does.contains(&Does::Edit) && proposal.edit.is_none() {
        return Err(format!(
            "{} offered an edit with nowhere to make it",
            gated.plugin
        ));
    }
    let choices = proposal
        .choices
        .into_iter()
        .map(|offered| QuestionChoice {
            label: offered.label,
            detail: offered.detail,
            recommended: false,
        })
        .collect();
    let approval = Approval {
        plugin: gated.plugin,
        method: gated.method,
        does,
        held: Some(Arc::new(Held {
            arguments,
            edit: proposal.edit,
        })),
    };
    questions::put_to_person(
        core,
        &agent_id,
        &request.project,
        proposal.header,
        proposal.text,
        choices,
        approval,
    )?;
    Ok(json!(
        "Sent to the person to approve. Stop here and end your turn without guessing: what they decide comes to you as a message."
    ))
}

/// Does what the person picked, answering what the agent is told was done. A typed answer
/// turns the call down.
pub(crate) async fn settle(
    core: &Arc<Core>,
    approval: &Approval,
    choice: Option<usize>,
) -> Result<String, String> {
    let Approval { plugin, method, .. } = approval;
    let held = || {
        approval
            .held
            .clone()
            .ok_or("Sikemux no longer holds this call; ask the agent to make it again")
    };
    match choice.and_then(|choice| approval.does.get(choice)) {
        Some(Does::Approve) => {
            let done = plugins::call(core, plugin, method, held()?.arguments.clone())
                .await
                .map_err(|failure| failure.message)?;
            plugins::tell_agents_if_theirs(core, plugin);
            plugins::wrote(core, plugin, method, &Value::Null);
            Ok(match done {
                Value::String(said) => format!("The person approved it. {said}"),
                _ => "The person approved it, and it is done.".to_owned(),
            })
        }
        Some(Does::Edit) => {
            let open = WindowCall::OpenPlugin {
                plugin: plugin.clone(),
                at: held()?.edit.clone().unwrap_or_default(),
            };
            let call = core
                .window
                .call(open, "change it in its form", false, None)?;
            let late = "Sikemux's window did not open the form; try again.".to_owned();
            super::harness::answer_of(core, call, CORE_REPLY_TIMEOUT, late).await?;
            Ok("The person opened it in Sikemux to change it first; nothing is done until they save it there.".to_owned())
        }
        Some(Does::Decline) | None => Ok("The person turned it down; nothing was done.".to_owned()),
    }
}
