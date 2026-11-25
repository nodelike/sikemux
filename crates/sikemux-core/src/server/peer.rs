//! Boards shared between people. A teammate's host reads a project this host shares with their
//! org over the peer channel, and this host reads the projects teammates share that it also
//! keeps, holding the last copy of each. Nothing on the channel acts on either host.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iroh::endpoint::{Connection, RecvStream, SendStream};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_client::join;
use sikemux_corekit::peer::{PeerHear, PeerSay, SharedBoard, SharedDecision, PEER_ALPN};
use sikemux_process::clock::{epoch_ms, epoch_secs};
use sikemux_process::sync::Locked;
use sikemux_view::task_list::{SharedBy, TaskOf, TaskRow, TeammateTask};
use sikemux_view::teammates::Teammate;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use crate::accounts::protocol::{PeerTicket, PeerTicketRequest, ShareRegistry};
use crate::cli::protocol::HarnessRequest;
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
use super::teammate_asks::{self, Asker};

const FILE: &str = "teammate-boards.json";
const OPENING: Duration = Duration::from_secs(10);
const LINGER: Duration = Duration::from_secs(2);
/// How long before a ticket ends the reading host asks for the next.
const RENEW_EARLY_SECS: i64 = 60;
const RETRY_FIRST: Duration = Duration::from_secs(5);
const RETRY_MOST: Duration = Duration::from_secs(300);
/// How long an agent here waits for a teammate's host to answer its question.
const ANSWERING: Duration = Duration::from_secs(120);

pub(crate) struct Peer {
    /// Moves on whenever a shared board, a share or an org changes, so open sessions look again.
    changed: watch::Sender<u64>,
    reading: Mutex<HashMap<Reading, Session>>,
    copies: Mutex<Copies>,
    members: Mutex<Members>,
    kept: Kept,
    pub(crate) shares: shares::Reconciling,
}

impl Peer {
    pub(crate) fn new(data_dir: Option<&Path>) -> Self {
        let kept = Kept::new(data_dir, FILE);
        Self {
            changed: watch::channel(0).0,
            reading: Mutex::default(),
            copies: Mutex::new(kept.load()),
            members: Mutex::default(),
            kept,
            shares: shares::Reconciling::new(data_dir),
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

/// An open session to a teammate's host, and the way to put a question on it.
struct Session {
    task: JoinHandle<()>,
    member: String,
    asks: mpsc::Sender<Asking>,
}

/// A question an agent here puts to a teammate's host, and where its answer goes.
struct Asking {
    task: u64,
    question: String,
    agent: String,
    cli: Option<String>,
    answer: oneshot::Sender<Result<Value, String>>,
}

/// Who the account's orgs hold, as last heard, and who was heard to leave them. A ticket outlives
/// its member's seat by minutes, so one that left is turned away by this rather than by its ticket.
#[derive(Default)]
struct Members {
    held: HashMap<String, HashSet<String>>,
    left: HashSet<(String, String)>,
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
    let (answered, mut answers) = mpsc::channel::<PeerHear>(4);
    loop {
        if !shares::active(core, &ticket.org) {
            return Some(refused("the org's Team plan is unpaid"));
        }
        if has_left(core, &ticket.org, &ticket.member) {
            return Some(refused("they are no longer in the org"));
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
                PeerSay::Ask { id, task, question, agent, cli } => {
                    let folder = (asked.as_deref()).and_then(|project| shared_folder(core, &ticket.org, project));
                    let Some(folder) = folder else {
                        return Some(PeerHear::Gone);
                    };
                    let asker = Asker {
                        org: ticket.org.clone(),
                        member: ticket.member.clone(),
                        name: ticket.member_name.clone(),
                        picture: None,
                        agent,
                        agent_cli: cli,
                    };
                    let (core, answered) = (core.clone(), answered.clone());
                    tokio::spawn(async move {
                        let frame = match teammate_asks::ask(&core, asker, folder, task, question).await {
                            Ok(said) => PeerHear::Answered { id, said },
                            Err(reason) => PeerHear::NotAnswered { id, reason },
                        };
                        let _ = answered.send(frame).await;
                    });
                }
            },
            Some(frame) = answers.recv() => {
                join::send(writer, &frame).await.ok()?;
            }
            _ = changes.changed() => {}
            () = tokio::time::sleep(Duration::from_secs(left)) => {
                return Some(refused("the ticket ran out"));
            }
        }
    }
}

fn has_left(core: &Core, org: &str, member: &str) -> bool {
    let members = core.peer().members.locked();
    members.left.contains(&(org.to_owned(), member.to_owned()))
}

/// Takes in who `registry`'s orgs hold now, and has open sessions look again when someone left.
fn note_members(core: &Core, registry: &ShareRegistry) {
    let mut left = false;
    let mut members = core.peer().members.locked();
    for list in &registry.orgs {
        let holds: HashSet<String> = list.members.iter().map(|one| one.id.clone()).collect();
        let held = members.held.remove(&list.org).unwrap_or_default();
        for gone in held.difference(&holds) {
            left |= members.left.insert((list.org.clone(), gone.clone()));
        }
        members.held.insert(list.org.clone(), holds);
    }
    drop(members);
    if left {
        changed(core);
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
    let folder = shared_folder(core, org, project)?;
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

/// The folder of the project known by `project`, while this host keeps its record and shares it
/// with `org`.
fn shared_folder(core: &Core, org: &str, project: &str) -> Option<String> {
    let folder = (core.workspaces().projects().into_iter())
        .filter(|known| core.hosts().copies.home_of(core, known).is_none())
        .find(|known| known.identity.as_deref().map(shares::key).as_deref() == Some(project))?
        .path
        .to_string_lossy()
        .into_owned();
    (shares::shared_with(core, &folder).as_deref() == Some(org)).then_some(folder)
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
/// paid, and closes those no longer shared. A board taken back is forgotten, as is every board of
/// an org the account has left; one whose org is unpaid is kept as last read.
async fn read_teammates(core: &Arc<Core>) {
    let Some(shared) = registry::read(core).await else {
        return;
    };
    note_members(core, &shared);
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
    let orgs = core.account().orgs();
    let forgot = forget(core, |copy| {
        let Some(orgs) = &orgs else {
            return false;
        };
        match orgs.iter().find(|org| org.id == copy.org) {
            None => true,
            Some(org) => {
                org.active
                    && !listed.contains(&(
                        copy.org.clone(),
                        copy.person.member.clone(),
                        copy.project.clone(),
                    ))
            }
        }
    });
    if forgot {
        core.changed(What::Read(Topic::Boards));
    }
    let mut reading = core.peer().reading.locked();
    reading.retain(|open, session| {
        let keep = wanted.contains_key(open) && !session.task.is_finished();
        if !keep {
            session.task.abort();
        }
        keep
    });
    for (open, person) in wanted {
        if reading.contains_key(&open) {
            continue;
        }
        let core = core.clone();
        let member = person.member.clone();
        let (asks, mut asked) = mpsc::channel(4);
        let task = tokio::spawn({
            let open = open.clone();
            async move { session(&core, open, person, &mut asked).await }
        });
        reading.insert(open, Session { task, member, asks });
    }
}

enum Ended {
    /// The project is no longer shared, or the teammate's host refused: wait for the registry.
    Over,
    Lost {
        heard: bool,
    },
}

async fn session(
    core: &Arc<Core>,
    open: Reading,
    person: Teammate,
    asked: &mut mpsc::Receiver<Asking>,
) {
    let mut wait = RETRY_FIRST;
    loop {
        match read_once(core, &open, &person, asked).await {
            Ended::Over => return,
            Ended::Lost { heard } => {
                if heard {
                    wait = RETRY_FIRST;
                }
                let retry = tokio::time::sleep(wait);
                tokio::pin!(retry);
                loop {
                    tokio::select! {
                        () = &mut retry => break,
                        Some(unanswered) = asked.recv() => drop(unanswered),
                    }
                }
                wait = (wait * 2).min(RETRY_MOST);
            }
        }
    }
}

async fn read_once(
    core: &Arc<Core>,
    open: &Reading,
    person: &Teammate,
    asked: &mut mpsc::Receiver<Asking>,
) -> Ended {
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
    let mut waiting: HashMap<u64, oneshot::Sender<Result<Value, String>>> = HashMap::new();
    let mut next_id = 0;
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
                Some(PeerHear::Answered { id, said }) => {
                    if let Some(answer) = waiting.remove(&id) {
                        let _ = answer.send(Ok(said));
                    }
                }
                Some(PeerHear::NotAnswered { id, reason }) => {
                    if let Some(answer) = waiting.remove(&id) {
                        let _ = answer.send(Err(reason));
                    }
                }
            },
            Some(asking) = asked.recv() => {
                next_id += 1;
                let ask = PeerSay::Ask {
                    id: next_id,
                    task: asking.task,
                    question: asking.question,
                    agent: asking.agent,
                    cli: asking.cli,
                };
                if join::send(&mut writer, &ask).await.is_err() {
                    return Ended::Lost { heard: got };
                }
                waiting.insert(next_id, asking.answer);
            }
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

/// The kept copies of teammates' boards for the repository of the project at `folder`, without
/// those of an org this host doesn't show.
fn copies_of(core: &Core, folder: &str) -> Vec<Copy> {
    let Some(project) = (core.workspaces().projects().into_iter())
        .find(|known| known.path.as_os_str() == folder)
        .and_then(|known| known.identity.as_deref().map(shares::key))
    else {
        return Vec::new();
    };
    let owner = core.remote().owner();
    let mut found: Vec<Copy> = {
        let copies = core.peer().copies.locked();
        if copies.owner != owner {
            return Vec::new();
        }
        (copies.boards.iter())
            .filter(|copy| copy.project == project)
            .cloned()
            .collect()
    };
    found.retain(|copy| !shares::hidden(core, &copy.org));
    found
}

/// The tasks on teammates' boards for the repository of the project at `folder`, in the order
/// of the person's own list.
pub(crate) fn tasks(core: &Core, folder: &str) -> Vec<TeammateTask> {
    let mut tasks: Vec<TeammateTask> = (copies_of(core, folder).iter())
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

/// Each org whose members share the repository of the project at `folder` with the person, and
/// those of them who have tasks on it.
pub(crate) fn shared_by(core: &Core, folder: &str) -> Vec<SharedBy> {
    let mut orgs: Vec<SharedBy> = Vec::new();
    for copy in copies_of(core, folder) {
        if copy.board.tasks.is_empty() {
            continue;
        }
        if let Some(org) = orgs.iter_mut().find(|org| org.org == copy.org) {
            org.people.push(copy.person);
            continue;
        }
        let Some(name) = shares::org_name(core, &copy.org) else {
            continue;
        };
        orgs.push(SharedBy {
            can_ask: shares::active(core, &copy.org),
            org: copy.org,
            name,
            people: vec![copy.person],
        });
    }
    orgs
}

/// Clears `can_ask` for each org that has turned teammates' agents asking off.
pub(crate) async fn heed_asking(core: &Core, orgs: &mut [SharedBy]) {
    for org in orgs.iter_mut().filter(|org| org.can_ask) {
        org.can_ask = crate::server::account::org_policy(core, &org.org)
            .await
            .asking;
    }
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

/// Teammates' tasks on the project at `folder`, each named the way an agent asks about it: the
/// person's name, or their member id when two share a name, then `#` and the task's number.
pub(crate) fn named_tasks(core: &Core, folder: &str) -> Vec<(String, TeammateTask)> {
    let tasks = tasks(core, folder);
    let named = |task: &TeammateTask| {
        let same = (tasks.iter()).any(|other| {
            other.person.name == task.person.name && other.person.member != task.person.member
        });
        let who = if same {
            &task.person.member
        } else {
            &task.person.name
        };
        match task.row.of {
            TaskOf::Task { id } => Some(format!("{who}#{id}")),
            TaskOf::Call { .. } => None,
        }
    };
    (tasks.iter())
        .filter_map(|task| Some((named(task)?, task.clone())))
        .collect()
}

/// The teammate's task `named` names on the project at `folder`, as [`named_tasks`] names it.
pub(crate) fn teammate_task(core: &Core, folder: &str, named: &str) -> Option<TeammateTask> {
    let named = named.trim().trim_start_matches('@');
    (named_tasks(core, folder).into_iter())
        .find(|(name, _)| name.eq_ignore_ascii_case(named))
        .map(|(_, task)| task)
}

/// The teammate's task an agent of theirs named `agent` holds on the project at `folder`.
pub(crate) fn held_by_teammate(core: &Core, folder: &str, agent: &str) -> Option<String> {
    let agent = agent.trim().trim_start_matches('@');
    (named_tasks(core, folder).into_iter())
        .find(|(_, task)| {
            (task.row.holder.as_ref()).is_some_and(|holder| holder.name.eq_ignore_ascii_case(agent))
        })
        .map(|(name, task)| {
            format!(
                "@{agent} is {}'s agent, on their task {name}",
                task.person.name
            )
        })
}

/// Puts `question` about the teammate's `task` to the host that keeps it, answering what that
/// host said, or the task as last heard when it can't be reached.
pub(crate) async fn ask(
    core: &Core,
    folder: &str,
    task: &TeammateTask,
    question: String,
    agent: String,
    cli: Option<String>,
) -> Result<Value, String> {
    let TaskOf::Task { id } = task.row.of else {
        return Err("Only a teammate's task can be asked about".into());
    };
    let project = (core.workspaces().projects().into_iter())
        .find(|known| known.path.as_os_str() == folder)
        .and_then(|known| known.identity.as_deref().map(shares::key));
    let asks = project.and_then(|project| {
        (core.peer().reading.locked().iter())
            .find(|(open, session)| open.project == project && session.member == task.person.member)
            .map(|(_, session)| session.asks.clone())
    });
    let name = &task.person.name;
    let heard = |line: String| {
        let ago = epoch_ms().saturating_sub(task.heard) / 60_000;
        json!({
            "task": task.row,
            "note": format!("{line} This is their task as last heard {ago} min ago."),
        })
    };
    let Some(asks) = asks else {
        return Ok(heard(format!(
            "{name}'s host isn't reachable now, so nothing answered."
        )));
    };
    let (answer, answered) = oneshot::channel();
    let asking = Asking {
        task: id,
        question,
        agent,
        cli,
        answer,
    };
    if asks.send(asking).await.is_err() {
        return Ok(heard(format!(
            "{name}'s host isn't reachable now, so nothing answered."
        )));
    }
    match tokio::time::timeout(ANSWERING, answered).await {
        Ok(Ok(said)) => said,
        Ok(Err(_)) => Ok(heard(format!(
            "{name}'s host isn't reachable now, so nothing answered."
        ))),
        Err(_) => Ok(heard(format!("{name}'s host did not answer in time."))),
    }
}

/// A whole task list for an agent, with the tasks on teammates' boards for the same project after
/// its own.
pub(crate) fn with_teammates(
    core: &Core,
    request: &HarnessRequest,
    listed: Result<Value, String>,
) -> Result<Value, String> {
    let mut listed = listed?;
    if request
        .params
        .get("task")
        .is_some_and(|task| !task.is_null())
    {
        return Ok(listed);
    }
    let project = super::caller::project_of(core, request);
    let theirs: Vec<Value> = (named_tasks(core, &project).into_iter())
        .map(|(name, task)| {
            json!({
                "task": name,
                "title": task.row.title,
                "section": task.row.section,
                "holder": task.row.holder.map(|holder| format!("@{}", holder.name)),
            })
        })
        .collect();
    if let (false, Some(listed)) = (theirs.is_empty(), listed.as_object_mut()) {
        listed.insert("teammates".into(), Value::Array(theirs));
        listed.insert(
            "teammatesNote".into(),
            "Teammates' tasks, read-only: agent_read with one and a question asks about it.".into(),
        );
    }
    Ok(listed)
}
