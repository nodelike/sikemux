//! What pairing, the account, the vault, sharing and notifying ask of the features built over
//! them. They cannot name those features, so a core is built with these, one function for each.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use iroh::endpoint::Connection;
use sikemux_calls::hosts::{HostRow, LinkedHost};
use sikemux_calls::remote::Admission;
use sikemux_plugin_api::vault::Book;
use sikemux_view::inbox::InboxItem;
use sikemux_view::push::ActivityGlance;
use sikemux_view::workspace::LookReply;

use sikemux_view::calls::PersonCall;

use super::{Core, CoreResult};
use crate::accounts::protocol::{AccountEvent, DeviceList, ShareRegistry};

/// A link being served until it ends.
type Serving = Pin<Box<dyn Future<Output = ()> + Send>>;
/// A plugin's name, the folder it keeps its sign-ins in, and how it reads them.
type VaultBook = (String, PathBuf, &'static dyn Book);

pub struct RemoteAbove {
    /// Makes sure the connection runs while this host is signed in. The server closes it at once
    /// when the host has no webhooks.
    pub hooks_connect: fn(&Arc<Core>),
    /// Remote access went off: every link connection closes with it.
    pub hosts_halt: fn(&Core),
    /// The person here removed the link with `host`: the other host hears of it
    /// if it can, and the agents working across the link are let go.
    pub hosts_unlink: fn(&Core, &str),
    /// A device or host was let in here: the linked hosts are told, so each keeps a copy.
    pub hosts_admitted: fn(&Core, &Admission),
    /// The person removed a device the account does not list: the linked hosts take it off too.
    pub hosts_remove_device: fn(&Core, &str),
    /// `host` is no longer linked with this one, for the reason `why`: what
    /// waits for it is dropped, and the leads of workers there are told.
    pub hosts_unlinked: fn(&Core, &str, &str),
    /// The open call the question `id` raised or joined.
    pub person_calls_asked_by: fn(&Core, &str) -> Option<PersonCall>,
    /// The call `id` while it waits on the person.
    pub person_calls_open_call: fn(&Core, &str) -> Option<PersonCall>,
    /// The open calls only this module tells the person of: those not raised by a question or a
    /// held action, which are told of as they always were.
    pub person_calls_raised_open: fn(&Core) -> Vec<PersonCall>,
    /// Whether the agent ends its turn on a wake it set itself: CI, a one-time timer, or a worker
    /// of its at work.
    pub turn_state_set_a_wake: fn(&Core, &str) -> bool,
    /// Linked hosts are dialled again at once.
    pub hosts_retry_now: fn(&Core),
    /// Shared pages waiting to upload try again at once.
    pub page_shares_retry_now: fn(&Core),
    /// The host signed in to an account, so shared pages waiting on sign-in try again.
    pub page_shares_signed_in: fn(&Core),
    /// The links to other hosts are looked at again.
    pub hosts_wake: fn(&Core),
    /// The hosts of the account are linked again.
    pub hosts_relink: fn(&Core),
    /// A linked host connected: its link is served until it ends.
    pub serve_link: fn(Arc<Core>, Connection) -> Serving,
    /// Answers a teammate's host reading a project shared with their org.
    pub serve_peer: fn(Arc<Core>, Connection) -> Serving,
    /// What this host draws behind its panes, for a browser on the account.
    pub host_look: fn(&Core) -> LookReply,
    /// The account no longer has these hosts, so their links go.
    pub forget_hosts_off_account: fn(&Core, &DeviceList) -> CoreResult<()>,
    /// The hosts this one lists, linked or found over SSH.
    pub host_rows: fn(&Core) -> Vec<HostRow>,
    /// The hosts linked with this one, as its devices are shown them.
    pub linked_hosts: fn(&Core) -> Vec<LinkedHost>,
    /// A newer release is looked for soon.
    pub check_updates_soon: fn(&Core),
    /// Each plugin's sign-ins for the vault, with the folder the plugin keeps them in.
    pub vault_books: fn(&Core) -> Vec<VaultBook>,
    /// The account said these things happened; the stash and the mods take what is theirs.
    pub account_heard: fn(&Arc<Core>, &[AccountEvent]),
    /// The account's share registry or orgs were read again, with the registry if it answered.
    pub shares_heard: fn(&Arc<Core>, Option<ShareRegistry>),
    /// The live connection is up again: the stash and the mods read what the account kept while
    /// this host was away.
    pub account_catch_up: fn(&Arc<Core>),
    /// What the inbox tells the person of now.
    pub inbox_told: fn(&Core) -> Vec<InboxItem>,
    /// What a phone's lock screen shows of this host at a glance.
    pub activity_glance: fn(&Core) -> ActivityGlance,
    /// Whether the agent waits on an answer to a question it asked.
    pub waits_on_answer: fn(&Core, &str) -> bool,
}
