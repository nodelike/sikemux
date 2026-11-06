//! The one way to say the core's state changed. What changed decides who hears: clients that
//! read it through a `Call`, the device view, the agents' wakes and the notifier. A burst of
//! changes to one topic and project goes out as one notice.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sikemux_process::sync::Locked;
use tokio::sync::Notify;

pub(crate) use sikemux_wire::changed::Topic;

use sikemux_wire::protocol::{encode_control, Event, ServerMessage};

use super::notify::Signal;
use super::Core;

const GATHER: Duration = Duration::from_millis(50);
/// Past this many paths a notice says anything may have changed.
const MAX_PATHS: usize = 512;

/// Paths by topic and project; none means anything may have changed.
type Pending = BTreeMap<(Topic, Option<String>), Option<BTreeSet<String>>>;

pub(crate) enum What<'a> {
    /// Something only a device's view of this host shows. The view goes out again shortly,
    /// with any changes that follow it.
    View,
    /// Something the person did changed the view, such as the tab in front, so it goes out at
    /// once: a window shows its tabs as the view says.
    ViewNow,
    /// Something the messages waiting for agents, their questions and the board depend on: an
    /// agent came to rest, started, went away, or the window opened or closed.
    Agents,
    Read(Topic),
    /// What clients read under this topic, for one project or all of them; `paths` names what
    /// moved inside it, and none means anything may have.
    ReadIn(Topic, Option<&'a str>, Option<Vec<String>>),
    Event(&'a Event),
    /// A chat's state, for the apps. Its permission requests already notify, so it stays out
    /// of the notices.
    ChatState(&'a Event),
}

impl Core {
    pub(crate) fn changed(&self, what: What<'_>) {
        match what {
            What::View => self.view_moved(),
            What::ViewNow => {
                self.device_view.due.notify_one();
                self.view_moved();
            }
            What::Agents => {
                self.harness.mailbox.wake.now();
                self.harness.questions.wake.now();
                self.boards.wake.now();
                self.updates.chats_changed();
                self.view_moved();
            }
            What::Read(topic) => self.read_changed(topic, None, None),
            What::ReadIn(topic, project, paths) => self.read_changed(topic, project, paths),
            What::Event(event) => {
                if let Some(signal) = signal_of(event) {
                    self.notify.send(signal);
                }
                self.tell(event);
            }
            What::ChatState(event) => self.tell(event),
        }
    }

    /// The view is built afresh, and with it what the inbox is made from may have moved.
    fn view_moved(&self) {
        self.device_view.stale.notify_one();
        self.notify.inbox_moved();
    }

    fn read_changed(&self, topic: Topic, project: Option<&str>, paths: Option<Vec<String>>) {
        if shown_on_devices(topic) {
            self.view_moved();
        }
        self.changes.changed(topic, project, paths);
    }

    fn tell(&self, event: &Event) {
        let Ok(frame) = encode_control(&ServerMessage::Event {
            event: event.clone(),
        }) else {
            return;
        };
        let frame: Arc<[u8]> = frame.into();
        let devices_hear = devices_hear(event);
        for client in self.clients() {
            if devices_hear || client.peer.is_local() {
                client.send(frame.clone());
            }
        }
        if view_shows(event) {
            self.view_moved();
        }
    }
}

fn shown_on_devices(topic: Topic) -> bool {
    matches!(topic, Topic::Account | Topic::Plugins | Topic::Preferences)
}

fn signal_of(event: &Event) -> Option<Signal> {
    match event {
        Event::Attention { attention } => Some(Signal::Attention(Box::new(attention.clone()))),
        Event::AttentionCleared { id, agent_id } => Some(Signal::Cleared {
            id: id.clone(),
            agent_id: agent_id.clone(),
        }),
        Event::AgentState(state) => Some(Signal::AgentState {
            agent_id: state.agent_id.clone(),
            state: state.state,
        }),
        _ => None,
    }
}

/// Paired devices hear only of terminals ending and changing, and that a read changed; the
/// rest reaches them in the device view. That the core is stopping is for the programs on
/// this host, which would otherwise start it again.
fn devices_hear(event: &Event) -> bool {
    match event {
        Event::Exited { .. } | Event::ShellMetadata(_) => true,
        Event::Changed { topic, .. } => *topic != Topic::Stopping,
        _ => false,
    }
}

/// Whether the device view shows what `event` tells of. What it shows that no event tells of
/// reaches it through [`What::View`].
fn view_shows(event: &Event) -> bool {
    match event {
        Event::Exited { .. }
        | Event::SshLink { .. }
        | Event::AgentState(_)
        | Event::Attention { .. }
        | Event::AttentionCleared { .. }
        | Event::ChatBegun { .. }
        | Event::TerminalTitle { .. } => true,
        Event::ShellMetadata(_)
        | Event::TaskOutput { .. }
        | Event::Chat { .. }
        | Event::DeviceView { .. }
        | Event::Remote { .. }
        | Event::Network { .. }
        | Event::FocusAgent { .. }
        | Event::Notice { .. }
        | Event::AccountDevice { .. }
        | Event::Changed { .. }
        | Event::Stream { .. } => false,
    }
}

#[derive(Default)]
pub(crate) struct Changes {
    pending: Mutex<Pending>,
    notify: Notify,
}

impl Changes {
    /// `topic` changed for `project`, or for every project when it is none.
    /// `paths` names what moved inside it; none means anything may have.
    fn changed(&self, topic: Topic, project: Option<&str>, paths: Option<Vec<String>>) {
        let mut pending = self.pending.locked();
        let key = (topic, project.map(str::to_owned));
        let known = pending.get(&key);
        let merged = match (known, paths) {
            (Some(None), _) | (_, None) => None,
            (Some(Some(known)), Some(paths)) => {
                let mut known = known.clone();
                known.extend(paths);
                Some(known)
            }
            (None, Some(paths)) => Some(paths.into_iter().collect()),
        };
        pending.insert(key, merged.filter(|paths| paths.len() <= MAX_PATHS));
        drop(pending);
        self.notify.notify_one();
    }
}

pub(crate) async fn serve(core: Arc<Core>) {
    loop {
        core.changes.notify.notified().await;
        tokio::time::sleep(GATHER).await;
        let pending = std::mem::take(&mut *core.changes.pending.locked());
        for ((topic, project), paths) in pending {
            core.changed(What::Event(&Event::Changed {
                topic,
                project,
                paths: paths.map(|paths| paths.into_iter().collect()),
            }));
        }
    }
}

impl super::part::Part for Changes {
    fn start(&self, core: &Arc<Core>) -> Vec<tokio::task::JoinHandle<()>> {
        vec![super::part::serving(core, serve)]
    }
}
