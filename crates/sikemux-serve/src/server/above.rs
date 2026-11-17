//! What the modules a core stands on call in the features built over them. They cannot name
//! those features, so a core is built with these, one function for each thing they ask.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use sikemux_grid::TerminalColors;
use sikemux_wire::protocol::{AgentState, Event, Request, RequestId, SessionId};
use tokio::sync::mpsc;

use super::access::{Peer, Rule};
use super::connection::{ClientConn, ClientId, FrameWriter};
use super::session::Session;
use super::{Core, CoreResult};

type Running<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
/// What a client asks, in the order it asked.
type Asked = mpsc::UnboundedReceiver<(RequestId, Request)>;

pub struct Above {
    /// The terminal theme's colours, which a screen answers a program's colour queries with.
    pub terminal_colors: fn(&Core) -> TerminalColors,
    /// A terminal started or ended, so which ports are taken may have changed.
    pub ports_changed: fn(&Core),
    /// An SSH terminal started, connected, dropped or ended.
    pub ssh_moved: fn(&Core),
    /// An SSH terminal reached the host `alias`.
    pub ssh_connected: fn(&Arc<Core>, &str),
    /// A task's terminal ended with this exit code or signal.
    pub task_ended: fn(&Core, SessionId, Option<u32>, Option<String>),
    /// An agent's terminal ended with this exit code or signal, in the middle of a turn or not.
    pub agent_exited: fn(&Core, &Session, Option<u32>, Option<&str>, bool),
    /// An agent in a terminal moved to this state from a busy one or not, because the person
    /// typed or not; with when it last wrote.
    pub agent_moved: fn(&Core, &str, AgentState, bool, bool, u64),
    /// The view goes to devices at once.
    pub view_due: fn(&Core),
    /// An agent came to rest, started or went away, or the window opened or closed.
    pub agents_moved: fn(&Core),
    /// The view is built afresh.
    pub view_moved: fn(&Core),
    /// Clients are told of this as it happens.
    pub told: fn(&Core, &Event),
    /// A request of the core's hand-over, answered on the connection it came on.
    pub frozen_request:
        for<'a> fn(&'a Arc<Core>, &'a Peer, &'a [u8], &'a mut FrameWriter) -> Running<'a>,
    /// A device just connected and is sent this host's view.
    pub send_device_view: fn(&Core, &ClientConn),
    /// Runs a client's requests as they come.
    pub run_requests: for<'a> fn(&'a Arc<Core>, &'a Arc<ClientConn>, Asked) -> Running<'a>,
    /// A client connected.
    pub client_joined: fn(&Core),
    /// A client's connection has gone.
    pub client_gone: fn(&Arc<Core>, ClientId),
    /// A task's output has been quiet long enough to say so.
    pub task_output_noticed: fn(&Arc<Core>, SessionId),
    /// Whether this client may do what `rule` guards.
    pub admit: fn(&Core, &ClientConn, &Rule) -> CoreResult<()>,
    /// Types a client's bytes into a terminal's program.
    pub write_to_program: fn(&Arc<Core>, &Arc<ClientConn>, RequestId, &Session, Vec<u8>),
}
