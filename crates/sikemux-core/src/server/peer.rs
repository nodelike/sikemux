//! Boards shared between people. A teammate's host reads a project this host shares with their
//! org over the peer channel, and this host reads the projects teammates share that it also
//! keeps, holding the last copy of each. Nothing on the channel acts on either host.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iroh::endpoint::{Connection, RecvStream, SendStream};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sikemux_client::join;
use sikemux_corekit::peer::{PeerHear, PeerSay, SharedBoard, SharedDecision, PEER_ALPN};
use sikemux_process::clock::{epoch_ms, epoch_secs};
use sikemux_process::sync::Locked;
use sikemux_view::task_list::{TaskRow, TeammateTask};
use sikemux_view::teammates::Teammate;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::accounts::protocol::{PeerTicket, PeerTicketRequest};
use crate::server::account::shares as registry;
use crate::server::agent_now::AgentNowCore;
use crate::server::board;
use crate::server::changed::{ChangedCore, Topic, What};
use crate::server::kept::Kept;
use crate::server::parts::Parts;
use crate::server::remote_state::RemoteParts;
use crate::server::workspace_state::WorkspaceParts;
use crate::server::Core;
use crate::sessions::asked::may_hold_secret;

use super::shares;

const FILE: &str = "teammate-boards.json";
const OPENING: Duration = Duration::from_secs(10);
const LINGER: Duration = Duration::from_secs(2);
/// How long before a ticket ends the reading host asks for the next.
const RENEW_EARLY_SECS: i64 = 60;
const RETRY_FIRST: Duration = Duration::from_secs(5);
const RETRY_MOST: Duration = Duration::from_secs(300);

pub(crate) struct Peer {
    /// Moves on whenever a shared board, a share or an org changes, so open sessions look again.
    changed: watch::Sender<u64>,
    reading: Mutex<HashMap<Reading, JoinHandle<()>>>,
    copies: Mutex<Copies>,
    kept: Kept,
}

impl Peer {
    pub(crate) fn new(data_dir: Option<&Path>) -> Self {
        let kept = Kept::new(data_dir, FILE);
        Self {
            changed: watch::channel(0).0,
            reading: Mutex::default(),
            copies: Mutex::new(kept.load()),
            kept,
        }
    }
}

/// A teammate's project this host reads, from the host that keeps its record.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Reading {
    org: String,
    host: String,
    project: String,
}

/// Teammates' boards as last read, for the account that read them.
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Copies {
    owner: Option<String>,
    boards: Vec<Copy>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Copy {
    org: String,
    person: Teammate,
    project: String,
    /// Unix milliseconds.
    heard: u64,
    board: SharedBoard,
}

/// Something changed that a teammate reading this host's boards may see differently.
pub(crate) fn changed(core: &Core) {
    core.peer().changed.send_modify(|count| *count += 1);
}

/// The account's registry or orgs changed: open sessions look again, and this host reads the
/// teammates' projects it keeps too.
pub(crate) fn heard(core: &Arc<Core>) {
    changed(core);
    let core = core.clone();
    tokio::spawn(async move { read_teammates(&core).await });
}

// The answering side.

pub(crate) async fn serve(core: Arc<Core>, connection: Connection) {
    let from = connection.remote_id().to_string();
    let Ok(Ok((mut writer, reader))) = tokio::time::timeout(OPENING, connection.accept_bi()).await
    else {
        return;
    };
    let mut said = frames::<PeerSay>(reader);
    let last = match tokio::time::timeout(OPENING, said.recv())
        .await
        .ok()
        .flatten()
    {
        Some(PeerSay::Ticket { ticket }) => match core.remote().check_peer(&ticket, &from) {
            Ok(()) => answer(&core, &from, ticket, &mut said, &mut writer).await,
            Err(refusal) => Some(refused(refusal.reason())),
        },
        _ => Some(refused("a session opens with a ticket")),
    };
    if let Some(last) = last {
        let _ = join::send(&mut writer, &last).await;
    }
    let _ = writer.finish();
    let _ = tokio::time::timeout(LINGER, connection.closed()).await;
}

/// Sends the asked board each time it changes, while the ticket and the org's plan hold, and
/// answers what to say last; none when the asking host went away.
async fn answer(
    core: &Arc<Core>,
    from: &str,
    ticket: PeerTicket,
    said: &mut mpsc::Receiver<PeerSay>,
    writer: &mut SendStream,
) -> Option<PeerHear> {
    let mut until = sikemux_corekit::peer::good_until(&ticket);
    let mut changes = core.peer().changed.subscribe();
    let mut asked: Option<String> = None;
    let mut sent: Option<SharedBoard> = None;
    loop {
        if !shares::active(core, &ticket.org) {
            return Some(refused("the org's Team plan is unpaid"));
        }
        if let Some(project) = &asked {
            let Some(board) = shared_board(core, &ticket.org, project) else {
                return Some(PeerHear::Gone);
            };
            if sent.as_ref() != Some(&board) {
                let frame = PeerHear::Board {
                    board: board.clone(),
                };
                join::send(writer, &frame).await.ok()?;
                sent = Some(board);
            }
        }
        let left = u64::try_from(until - now()).unwrap_or(0);
        tokio::select! {
            next = said.recv() => match next? {
                PeerSay::Ticket { ticket: next } => {
                    let same = next.org == ticket.org && next.member == ticket.member;
                    match core.remote().check_peer(&next, from) {
                        Ok(()) if same => until = sikemux_corekit::peer::good_until(&next),
                        Ok(()) => return Some(refused("a session keeps one org and one person")),
                        Err(refusal) => return Some(refused(refusal.reason())),
                    }
                }
                PeerSay::Read { project } => {
                    asked = Some(project);
                    sent = None;
                }
            },
            _ = changes.changed() => {}
            () = tokio::time::sleep(Duration::from_secs(left)) => {
                return Some(refused("the ticket ran out"));
            }
        }
    }
}

fn now() -> i64 {
    i64::try_from(epoch_secs()).unwrap_or(i64::MAX)
}

fn refused(reason: impl Into<String>) -> PeerHear {
    PeerHear::Refused {
        reason: reason.into(),
    }
}

/// The board of the project known by `project` as `org`'s members read it, while this host keeps
/// its record and shares it with them.
fn shared_board(core: &Core, org: &str, project: &str) -> Option<SharedBoard> {
    let folder = (core.workspaces().projects().into_iter())
        .filter(|known| core.hosts().copies.home_of(core, known).is_none())
        .find(|known| known.identity.as_deref().map(shares::key).as_deref() == Some(project))?
        .path
        .to_string_lossy()
        .into_owned();
    if shares::shared_with(core, &folder).as_deref() != Some(org) {
        return None;
    }
    let tasks = (board::task_rows(core, &folder).into_iter())
        .filter_map(|row| crossing(core, row))
        .collect();
    let decisions = (board::goal::standing(core, &folder).into_iter())
        .filter(|decision| !may_hold_secret(&decision.says()))
        .map(|decision| SharedDecision {
            id: decision.id,
            says: decision.says(),
            why: decision.why.filter(|why| !may_hold_secret(why)),
            agent: (decision.learned.agent.is_some())
                .then_some(decision.learned.name)
                .flatten(),
            person: decision.learned.person,
            at: decision.learned.at,
        })
        .collect();
    Some(SharedBoard { tasks, decisions })
}

/// `row` as it may cross to a teammate, naming the CLI its agent runs; none when its title may
/// hold a secret.
fn crossing(core: &Core, mut row: TaskRow) -> Option<TaskRow> {
    if may_hold_secret(&row.title) {
        return None;
    }
    if let Some(holder) = row.holder.as_mut() {
        holder.cli = core.agent_now(&holder.agent_id).cli();
    }
    if let Some(dropped) = row.dropped.as_mut() {
        if may_hold_secret(&dropped.why) {
            dropped.why.clear();
        }
        dropped.instead = dropped
            .instead
            .take()
            .filter(|title| !may_hold_secret(title));
    }
    Some(row)
}

// The reading side.

/// Opens a session to each teammate's project this host keeps too, in the orgs whose plan is
/// paid, and closes those no longer shared. A board taken back is forgotten; one whose org is
/// unpaid is kept as last read.
async fn read_teammates(core: &Arc<Core>) {
    let Some(shared) = registry::read(core).await else {
        return;
    };
    let keys: Vec<String> = (core.workspaces().projects().into_iter())
        .filter_map(|known| known.identity.as_deref().map(shares::key))
        .collect();
    let mut wanted: HashMap<Reading, Teammate> = HashMap::new();
    let mut listed: Vec<(String, String, String)> = Vec::new();
    for list in &shared.orgs {
        for share in &list.shares {
            listed.push((
                list.org.clone(),
                share.member.clone(),
                share.project.clone(),
            ));
            let Some(member) = (list.members.iter()).find(|member| member.id == share.member)
            else {
                continue;
            };
            if !keys.contains(&share.project) || !shares::active(core, &list.org) {
                continue;
            }
            let reading = Reading {
                org: list.org.clone(),
                host: share.host.clone(),
                project: share.project.clone(),
            };
            let person = Teammate {
                member: member.id.clone(),
                name: member.name.clone(),
                picture: member.image_url.clone(),
                you: false,
            };
            wanted.insert(reading, person);
        }
    }
    let forgot = forget(core, |copy| {
        shares::active(core, &copy.org)
            && !listed.contains(&(
                copy.org.clone(),
                copy.person.member.clone(),
                copy.project.clone(),
            ))
    });
    if forgot {
        core.changed(What::Read(Topic::Boards));
    }
    let mut reading = core.peer().reading.locked();
    reading.retain(|open, task| {
        let keep = wanted.contains_key(open) && !task.is_finished();
        if !keep {
            task.abort();
        }
        keep
    });
    for (open, person) in wanted {
        if reading.contains_key(&open) {
            continue;
        }
        let core = core.clone();
        let task = tokio::spawn({
            let open = open.clone();
            async move { session(&core, open, person).await }
        });
        reading.insert(open, task);
    }
}

enum Ended {
    /// The project is no longer shared, or the teammate's host refused: wait for the registry.
    Over,
    Lost {
        heard: bool,
    },
}

async fn session(core: &Arc<Core>, open: Reading, person: Teammate) {
    let mut wait = RETRY_FIRST;
    loop {
        match read_once(core, &open, &person).await {
            Ended::Over => return,
            Ended::Lost { heard } => {
                if heard {
                    wait = RETRY_FIRST;
                }
                tokio::time::sleep(wait).await;
                wait = (wait * 2).min(RETRY_MOST);
            }
        }
    }
}

async fn read_once(core: &Arc<Core>, open: &Reading, person: &Teammate) -> Ended {
    let lost = Ended::Lost { heard: false };
    let Some(mut ticket) = ticket(core, open).await else {
        return lost;
    };
    let Some(endpoint) = core.remote().endpoint() else {
        return lost;
    };
    let Ok(addr) = super::hosts::addr_of(core, &open.host, &[]) else {
        return Ended::Over;
    };
    let Ok(Ok(connection)) = tokio::time::timeout(OPENING, endpoint.connect(addr, PEER_ALPN)).await
    else {
        return lost;
    };
    let Ok((mut writer, reader)) = connection.open_bi().await else {
        return lost;
    };
    let read = PeerSay::Read {
        project: open.project.clone(),
    };
    let opening = PeerSay::Ticket {
        ticket: ticket.clone(),
    };
    if join::send(&mut writer, &opening).await.is_err()
        || join::send(&mut writer, &read).await.is_err()
    {
        return lost;
    }
    let mut heard = frames::<PeerHear>(reader);
    let mut got = false;
    loop {
        let renew = u64::try_from(ticket.expires_at - RENEW_EARLY_SECS - now()).unwrap_or(0);
        tokio::select! {
            next = heard.recv() => match next {
                None => return Ended::Lost { heard: got },
                Some(PeerHear::Board { board }) => {
                    got = true;
                    keep(core, open, person, board);
                }
                Some(PeerHear::Gone) => {
                    forget(core, |copy| {
                        copy.org == open.org && copy.person.member == person.member && copy.project == open.project
                    });
                    core.changed(What::Read(Topic::Boards));
                    return Ended::Over;
                }
                Some(PeerHear::Refused { reason }) => {
                    eprintln!("sikemux core: a teammate's host would not share its board: {reason}");
                    return Ended::Over;
                }
            },
            () = tokio::time::sleep(Duration::from_secs(renew)) => {
                let Some(next) = self::ticket(core, open).await else {
                    return Ended::Lost { heard: got };
                };
                ticket = next;
                let again = PeerSay::Ticket { ticket: ticket.clone() };
                if join::send(&mut writer, &again).await.is_err() {
                    return Ended::Lost { heard: got };
                }
            }
        }
    }
}

async fn ticket(core: &Core, open: &Reading) -> Option<PeerTicket> {
    let request = PeerTicketRequest {
        org: open.org.clone(),
        host: open.host.clone(),
    };
    match registry::ticket(core, &request).await {
        Ok(ticket) => Some(ticket),
        Err(error) => {
            eprintln!("sikemux core: no ticket to read a teammate's board: {error}");
            None
        }
    }
}

fn keep(core: &Core, open: &Reading, person: &Teammate, board: SharedBoard) {
    let owner = core.remote().owner();
    let mut copies = core.peer().copies.locked();
    if copies.owner != owner {
        *copies = Copies {
            owner,
            boards: Vec::new(),
        };
    }
    copies.boards.retain(|copy| {
        !(copy.org == open.org
            && copy.person.member == person.member
            && copy.project == open.project)
    });
    copies.boards.push(Copy {
        org: open.org.clone(),
        person: person.clone(),
        project: open.project.clone(),
        heard: epoch_ms(),
        board,
    });
    core.peer().kept.save_soon(&*copies);
    drop(copies);
    core.changed(What::Read(Topic::Boards));
}

/// Drops the copies `gone` picks, answering whether there were any.
fn forget(core: &Core, gone: impl Fn(&Copy) -> bool) -> bool {
    let mut copies = core.peer().copies.locked();
    let before = copies.boards.len();
    copies.boards.retain(|copy| !gone(copy));
    let forgot = copies.boards.len() != before;
    if forgot {
        core.peer().kept.save_soon(&*copies);
    }
    forgot
}

/// The tasks on teammates' boards for the repository of the project at `folder`, in the order
/// of the person's own list.
pub(crate) fn tasks(core: &Core, folder: &str) -> Vec<TeammateTask> {
    let Some(project) = (core.workspaces().projects().into_iter())
        .find(|known| known.path.as_os_str() == folder)
        .and_then(|known| known.identity.as_deref().map(shares::key))
    else {
        return Vec::new();
    };
    let owner = core.remote().owner();
    let copies = core.peer().copies.locked();
    if copies.owner != owner {
        return Vec::new();
    }
    let mut tasks: Vec<TeammateTask> = (copies.boards.iter())
        .filter(|copy| copy.project == project)
        .flat_map(|copy| {
            (copy.board.tasks.iter()).map(|row| TeammateTask {
                person: copy.person.clone(),
                heard: copy.heard,
                row: row.clone(),
            })
        })
        .collect();
    tasks.sort_by(|one, other| {
        (one.row.section.cmp(&other.row.section)).then(other.row.since.cmp(&one.row.since))
    });
    tasks
}

/// Each control frame read from `reader`, until it ends or sends something else.
fn frames<T: DeserializeOwned + Send + 'static>(mut reader: RecvStream) -> mpsc::Receiver<T> {
    let (send, receive) = mpsc::channel(8);
    tokio::spawn(async move {
        while let Ok(frame) = join::receive::<T>(&mut reader, Duration::MAX).await {
            if send.send(frame).await.is_err() {
                return;
            }
        }
    });
    receive
}
