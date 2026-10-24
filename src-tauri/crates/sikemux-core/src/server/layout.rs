//! The tabs desktop clients draw, kept by the core so every client shows the
//! same ones. Agents' tabs follow the workspace: a new agent gets a tab at the
//! end of its project's strip, and a tab goes once nothing it shows is left.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use serde_json::{json, Value};
use sikemux_process::sync::Locked;

use crate::cli::protocol::{CliOpenTarget, CliTargetKind};
use crate::protocol::{Layout, Pane, PaneKind, SessionId, SessionInfo, SplitDir, Tab, Workspace};

use super::kept::Kept;
use super::{Core, CoreError, CoreResult};

const FILE: &str = "layout.json";
const MAX_TABS: usize = 64;

pub(crate) struct Layouts {
    layout: Mutex<Layout>,
    kept: Kept,
    /// What each paired device has in front, by its id, once it moved anything there. The
    /// tabs are the same for every client; what is in front is each one's own.
    fronts: Mutex<HashMap<String, Front>>,
}

tokio::task_local! {
    /// The paired device whose request is being answered, so a layout edit it causes moves
    /// only what that device has in front.
    static ASKER: Option<String>;
}

/// Answers `request` for `device`, or for this host's own clients when there is none.
pub(crate) async fn asked_by<F: Future>(device: Option<String>, request: F) -> F::Output {
    ASKER.scope(device, request).await
}

fn asker() -> Option<String> {
    ASKER.try_with(Clone::clone).ok().flatten()
}

/// The project, host, strip and tabs a client has in front.
#[derive(Clone, Default)]
struct Front {
    active_project: Option<String>,
    active_host: Option<String>,
    chosen_host: Option<String>,
    terminals_in_front: bool,
    active: BTreeMap<String, Option<String>>,
}

impl Front {
    fn of(layout: &Layout) -> Self {
        let projects = layout
            .projects
            .iter()
            .map(|(id, strip)| (format!("project:{id}"), strip.active.clone()));
        let hosts = layout
            .hosts
            .iter()
            .map(|(alias, strip)| (format!("ssh:{alias}"), strip.active.clone()));
        Self {
            active_project: layout.active_project.clone(),
            active_host: layout.active_host.clone(),
            chosen_host: layout.chosen_host.clone(),
            terminals_in_front: layout.terminals_in_front,
            active: projects
                .chain(hosts)
                .chain([("terminals".to_owned(), layout.terminals.active.clone())])
                .collect(),
        }
    }

    fn lay_over(&self, layout: &mut Layout) {
        layout.active_project = self.active_project.clone();
        layout.active_host = self.active_host.clone();
        layout.chosen_host = self.chosen_host.clone();
        layout.terminals_in_front = self.terminals_in_front;
        let front = |key: String| self.active.get(&key).cloned();
        for (id, strip) in &mut layout.projects {
            if let Some(active) = front(format!("project:{id}")) {
                strip.active = active;
            }
        }
        for (alias, strip) in &mut layout.hosts {
            if let Some(active) = front(format!("ssh:{alias}")) {
                strip.active = active;
            }
        }
        if let Some(active) = front("terminals".to_owned()) {
            layout.terminals.active = active;
        }
    }
}

impl Layouts {
    pub(crate) fn new(data_dir: Option<&std::path::Path>) -> Self {
        let kept = Kept::new(data_dir, FILE);
        Self {
            layout: Mutex::new(kept.load()),
            kept,
            fronts: Mutex::default(),
        }
    }
}

/// The layout as `device` is sent it, or this host's own clients when there is none, fitted
/// to the workspace and sessions.
pub(crate) fn view(
    core: &Core,
    workspace: &Workspace,
    sessions: &[SessionInfo],
    device: Option<&str>,
) -> Layout {
    let mut layout = core.layouts.layout.locked().clone();
    if let Some(front) = device.and_then(|device| core.layouts.fronts.locked().get(device).cloned())
    {
        front.lay_over(&mut layout);
    }
    on_linked_host(core, fitted(layout, workspace, sessions))
}

/// The paired devices with what they have in front of their own.
pub(crate) fn fronted(core: &Core) -> HashSet<String> {
    core.layouts.fronts.locked().keys().cloned().collect()
}

/// The layout, with the chosen host dropped once it is no longer linked with this one.
fn on_linked_host(core: &Core, mut layout: Layout) -> Layout {
    if let Some(chosen) = &layout.chosen_host {
        if !core.remote.links().iter().any(|link| &link.id == chosen) {
            layout.chosen_host = None;
        }
    }
    layout
}

fn fitted(mut layout: Layout, workspace: &Workspace, sessions: &[SessionInfo]) -> Layout {
    let agents: HashSet<&str> = workspace
        .agents
        .iter()
        .map(|agent| agent.id.as_str())
        .collect();
    let terminals: HashSet<String> = sessions
        .iter()
        .map(|session| session.id.to_string())
        .collect();
    let shown = |kind: PaneKind, target: &str| match kind {
        PaneKind::Agent | PaneKind::Desk => agents.contains(target),
        PaneKind::Terminal => terminals.contains(target),
        _ => true,
    };
    layout
        .projects
        .retain(|id, _| workspace.projects.iter().any(|project| &project.id == id));
    for project in &workspace.projects {
        let strip = layout.projects.entry(project.id.clone()).or_default();
        strip
            .tabs
            .retain_mut(|tab| keep_shown(&mut tab.pane, &shown));
        let mut created: Vec<_> = workspace
            .agents
            .iter()
            .filter(|agent| agent.project == project.id)
            .filter(|agent| {
                !strip
                    .tabs
                    .iter()
                    .any(|tab| holds(&tab.pane, PaneKind::Agent, &agent.id))
            })
            .collect();
        created.sort_by_key(|agent| agent.created_at);
        for agent in created
            .into_iter()
            .take(MAX_TABS.saturating_sub(strip.tabs.len()))
        {
            strip.tabs.push(tab(
                format!("agent:{}", agent.id),
                PaneKind::Agent,
                agent.id.clone(),
            ));
        }
        let active_shown = strip
            .active
            .as_ref()
            .is_some_and(|active| strip.tabs.iter().any(|tab| &tab.id == active));
        if !active_shown {
            strip.active = strip.tabs.first().map(|tab| tab.id.clone());
        }
    }
    let active_shown = layout
        .active_project
        .as_ref()
        .is_some_and(|active| layout.projects.contains_key(active));
    if !active_shown {
        layout.active_project = workspace.projects.first().map(|project| project.id.clone());
    }
    layout.hosts.retain(|_, strip| {
        strip
            .tabs
            .retain_mut(|tab| keep_shown(&mut tab.pane, &shown));
        if !strip
            .active
            .as_ref()
            .is_some_and(|active| strip.tabs.iter().any(|tab| &tab.id == active))
        {
            strip.active = strip.tabs.first().map(|tab| tab.id.clone());
        }
        !strip.tabs.is_empty()
    });
    if !layout
        .active_host
        .as_ref()
        .is_some_and(|host| layout.hosts.contains_key(host))
    {
        layout.active_host = None;
    }
    let terminals = &mut layout.terminals;
    terminals
        .tabs
        .retain_mut(|tab| keep_shown(&mut tab.pane, &shown));
    if !terminals
        .active
        .as_ref()
        .is_some_and(|active| terminals.tabs.iter().any(|tab| &tab.id == active))
    {
        terminals.active = terminals.tabs.first().map(|tab| tab.id.clone());
    }
    layout.terminals_in_front &= !terminals.tabs.is_empty();
    if layout.terminals_in_front {
        layout.active_host = None;
    }
    layout
}

/// Drops the views nothing is left for, answering whether any remain.
fn keep_shown(pane: &mut Pane, shown: &impl Fn(PaneKind, &str) -> bool) -> bool {
    let keep = |view: &Pane| match view {
        Pane::View { kind, target, .. } => shown(*kind, target),
        Pane::Split { .. } => true,
    };
    keep(pane) && prune(pane, &keep)
}

/// Drops the views inside `pane` that `keep` refuses. A split left with one pane
/// becomes that pane, and its share goes to the panes that stay. Answers whether
/// anything remains.
fn prune(pane: &mut Pane, keep: &impl Fn(&Pane) -> bool) -> bool {
    let Pane::Split { panes, sizes, .. } = pane else {
        return true;
    };
    let mut kept = Vec::with_capacity(panes.len());
    let mut kept_sizes = Vec::with_capacity(panes.len());
    for (mut child, size) in panes.drain(..).zip(sizes.iter().copied()) {
        if keep(&child) && prune(&mut child, keep) {
            kept.push(child);
            kept_sizes.push(size);
        }
    }
    match kept.len() {
        0 => false,
        1 => {
            *pane = kept.remove(0);
            true
        }
        _ => {
            *panes = kept;
            *sizes = shares(&kept_sizes);
            true
        }
    }
}

/// `sizes` scaled to thousandths, every pane keeping some room.
fn shares(sizes: &[u16]) -> Vec<u16> {
    let total: u32 = sizes.iter().map(|&size| u32::from(size.max(1))).sum();
    let mut scaled: Vec<u16> = sizes
        .iter()
        .map(|&size| (u32::from(size.max(1)) * 1000 / total).max(1) as u16)
        .collect();
    let spare = 1000 - scaled.iter().map(|&size| i32::from(size)).sum::<i32>();
    if let Some(last) = scaled.last_mut() {
        *last = (i32::from(*last) + spare).max(1) as u16;
    }
    scaled
}

fn holds(pane: &Pane, kind: PaneKind, target: &str) -> bool {
    match pane {
        Pane::View {
            kind: held,
            target: held_target,
            ..
        } => *held == kind && held_target == target,
        Pane::Split { panes, .. } => panes.iter().any(|pane| holds(pane, kind, target)),
    }
}

fn view_id() -> String {
    format!("pane-{}", uuid::Uuid::new_v4().simple())
}

/// Puts `new` beside the view `beside`, answering whether that view is in `pane`.
/// A split already running the same way takes it as one more pane.
fn insert_beside(
    pane: &mut Pane,
    beside: &str,
    dir: SplitDir,
    after: bool,
    new: &mut Option<Pane>,
) -> bool {
    match pane {
        Pane::View { id, .. } if id == beside => {
            let Some(new) = new.take() else {
                return true;
            };
            let old = std::mem::replace(
                pane,
                Pane::Split {
                    id: view_id(),
                    dir,
                    panes: Vec::new(),
                    sizes: vec![500, 500],
                },
            );
            if let Pane::Split { panes, .. } = pane {
                *panes = if after {
                    vec![old, new]
                } else {
                    vec![new, old]
                };
            }
            true
        }
        Pane::View { .. } => false,
        Pane::Split {
            dir: running,
            panes,
            sizes,
            ..
        } => {
            let at = panes
                .iter()
                .position(|child| matches!(child, Pane::View { id, .. } if id == beside));
            if let (Some(at), true) = (at, *running == dir) {
                let Some(new) = new.take() else {
                    return true;
                };
                let half = sizes[at] / 2;
                sizes[at] -= half;
                let to = if after { at + 1 } else { at };
                panes.insert(to, new);
                sizes.insert(to, half);
                *sizes = shares(sizes);
                return true;
            }
            panes
                .iter_mut()
                .any(|child| insert_beside(child, beside, dir, after, new))
        }
    }
}

/// Puts `new` in place of the view `view`, answering whether that view is in `pane`.
fn replace_view(pane: &mut Pane, view: &str, new: &mut Option<Pane>) -> bool {
    match pane {
        Pane::View { id, .. } if id == view => {
            if let Some(new) = new.take() {
                *pane = new;
            }
            true
        }
        Pane::View { .. } => false,
        Pane::Split { panes, .. } => panes.iter_mut().any(|child| replace_view(child, view, new)),
    }
}

/// Shows the terminal `id` to the right of the view `pane` of `tab`, or in its place, and
/// brings the tab to the front. Answers the tab's id.
pub(crate) fn place_terminal(
    core: &Core,
    at: At,
    tab: String,
    pane: &str,
    id: SessionId,
    replace: bool,
) -> CoreResult<String> {
    edit(core, |layout| {
        let strip = strip_at(layout, &at)?;
        let held = tab_in(strip, &tab)?;
        let mut new = Some(Pane::View {
            id: view_id(),
            kind: PaneKind::Terminal,
            target: id.to_string(),
        });
        let placed = match replace {
            true => replace_view(&mut held.pane, pane, &mut new),
            false => insert_beside(&mut held.pane, pane, SplitDir::Row, true, &mut new),
        };
        if !placed {
            return Err("that view is not in the tab".into());
        }
        strip.active = Some(tab.clone());
        bring_forward(layout, &at);
        Ok(tab)
    })
}

fn find_split<'a>(pane: &'a mut Pane, split: &str) -> Option<&'a mut Pane> {
    match pane {
        Pane::Split { id, .. } if id == split => Some(pane),
        Pane::Split { panes, .. } => panes.iter_mut().find_map(|child| find_split(child, split)),
        Pane::View { .. } => None,
    }
}

fn find_view<'a>(pane: &'a Pane, view: &str) -> Option<&'a Pane> {
    match pane {
        Pane::View { id, .. } if id == view => Some(pane),
        Pane::View { .. } => None,
        Pane::Split { panes, .. } => panes.iter().find_map(|child| find_view(child, view)),
    }
}

fn agents_in(pane: &Pane, found: &mut Vec<String>) {
    match pane {
        Pane::View {
            kind: PaneKind::Agent,
            target,
            ..
        } => found.push(target.clone()),
        Pane::View { .. } => {}
        Pane::Split { panes, .. } => panes.iter().for_each(|pane| agents_in(pane, found)),
    }
}

fn tab(id: String, kind: PaneKind, target: String) -> Tab {
    Tab {
        id: id.clone(),
        title: None,
        pane: Pane::View { id, kind, target },
        preview: false,
    }
}

/// Changes the layout as clients see it now, keeps it and tells them.
/// An edit a paired device asked for moves only what it has in front; the tabs it opens,
/// closes or moves change for everyone.
fn edit<T>(core: &Core, change: impl FnOnce(&mut Layout) -> CoreResult<T>) -> CoreResult<T> {
    let workspace = core.workspace_view();
    let sessions = core.session_infos();
    let device = asker();
    let answer = {
        let mut kept = core.layouts.layout.locked();
        let mut fronts = core.layouts.fronts.locked();
        let shared = on_linked_host(core, fitted(kept.clone(), &workspace, &sessions));
        let mut layout = shared.clone();
        if let Some(front) = device.as_ref().and_then(|device| fronts.get(device)) {
            front.lay_over(&mut layout);
        }
        let answer = change(&mut layout)?;
        if let Some(device) = device {
            fronts.insert(device, Front::of(&layout));
            Front::of(&shared).lay_over(&mut layout);
        }
        let paired: HashSet<String> = core
            .remote
            .status()
            .devices
            .into_iter()
            .map(|paired| paired.id)
            .collect();
        fronts.retain(|device, _| paired.contains(device));
        *kept = layout;
        core.layouts.kept.save(&*kept);
        answer
    };
    core.view_changed_now();
    core.spares.front_changed();
    Ok(answer)
}

/// The project the person has in front, by id, as the layout last said.
pub(crate) fn front_project(core: &Core) -> Option<String> {
    core.layouts.layout.locked().active_project.clone()
}

fn strip<'a>(
    layout: &'a mut Layout,
    project: &str,
) -> CoreResult<&'a mut crate::protocol::ProjectLayout> {
    layout
        .projects
        .get_mut(project)
        .ok_or_else(|| CoreError::from("that project is not on this host"))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OpenTab {
    #[serde(flatten)]
    at: At,
    kind: PaneKind,
    target: String,
    title: Option<String>,
    /// Opens it to look at, in place of the strip's tab opened that way.
    #[serde(default)]
    preview: bool,
    /// Opens it in front of its strip but leaves the strip in front where it is.
    #[serde(default)]
    behind: bool,
}

/// Brings the tab showing `target` to the front, opening one at the end of
/// the strip when none does, or in place of the strip's preview when it
/// opens to look at. A tab brought forward to keep is kept. Answers its id.
pub(crate) fn open_tab(core: &Core, params: OpenTab) -> CoreResult<String> {
    edit(core, |layout| {
        let strip = strip_at(layout, &params.at)?;
        let shown = strip
            .tabs
            .iter()
            .position(|tab| holds(&tab.pane, params.kind, &params.target));
        let replaced = strip.tabs.iter().position(|tab| tab.preview);
        let id = match (shown, replaced) {
            (Some(at), _) => {
                let tab = &mut strip.tabs[at];
                tab.preview &= params.preview;
                tab.id.clone()
            }
            (None, Some(at)) if params.preview => {
                let tab = &mut strip.tabs[at];
                let view = format!("view-{}", uuid::Uuid::new_v4().simple());
                tab.pane = Pane::View {
                    id: view,
                    kind: params.kind,
                    target: params.target,
                };
                tab.title = params.title;
                tab.id.clone()
            }
            (None, _) => {
                if strip.tabs.len() >= MAX_TABS {
                    return Err("that strip already has as many tabs as the core keeps".into());
                }
                let mut opened = tab(
                    format!("tab-{}", uuid::Uuid::new_v4().simple()),
                    params.kind,
                    params.target,
                );
                opened.title = params.title;
                opened.preview = params.preview;
                let id = opened.id.clone();
                strip.tabs.push(opened);
                id
            }
        };
        strip.active = Some(id.clone());
        if !params.behind {
            bring_forward(layout, &params.at);
        }
        Ok(id)
    })
}

/// The strip a call names: the terminals of no project, an SSH host's, or a project's.
#[derive(Clone, Default, Deserialize)]
pub(crate) struct At {
    #[serde(default)]
    pub(crate) project: String,
    pub(crate) host: Option<String>,
    #[serde(default)]
    pub(crate) terminals: bool,
}

impl At {
    pub(crate) fn project(project: &str) -> Self {
        Self {
            project: project.to_owned(),
            ..Self::default()
        }
    }

    pub(crate) fn terminals() -> Self {
        Self {
            terminals: true,
            ..Self::default()
        }
    }
}

fn strip_at<'a>(
    layout: &'a mut Layout,
    at: &At,
) -> CoreResult<&'a mut crate::protocol::ProjectLayout> {
    match (&at.host, at.terminals) {
        (_, true) => Ok(&mut layout.terminals),
        (Some(host), false) => layout
            .hosts
            .get_mut(host)
            .ok_or_else(|| CoreError::from("no terminal is open to that host")),
        (None, false) => strip(layout, &at.project),
    }
}

/// Puts the strip `at` names in front.
fn bring_forward(layout: &mut Layout, at: &At) {
    layout.chosen_host = None;
    layout.terminals_in_front = at.terminals;
    match (&at.host, at.terminals) {
        (_, true) => layout.active_host = None,
        (Some(host), false) => layout.active_host = Some(host.clone()),
        (None, false) => {
            layout.active_project = Some(at.project.clone());
            layout.active_host = None;
        }
    }
}

fn tab_in<'a>(strip: &'a mut crate::protocol::ProjectLayout, tab: &str) -> CoreResult<&'a mut Tab> {
    strip
        .tabs
        .iter_mut()
        .find(|held| held.id == tab)
        .ok_or_else(|| CoreError::from("that tab is not open"))
}

/// Opens a tab on the terminal `id` at the end of the SSH host's strip and
/// brings the host to the front. Answers the tab's id.
pub(crate) fn show_ssh_terminal(core: &Core, host: String, id: SessionId) -> CoreResult<String> {
    edit(core, |layout| {
        let strip = layout.hosts.entry(host.clone()).or_default();
        if strip.tabs.len() >= MAX_TABS {
            return Err("that host already has as many tabs as the core keeps".into());
        }
        let opened = tab(
            format!("tab-{}", uuid::Uuid::new_v4().simple()),
            PaneKind::Terminal,
            id.to_string(),
        );
        let tab_id = opened.id.clone();
        strip.tabs.push(opened);
        strip.active = Some(tab_id.clone());
        layout.active_host = Some(host);
        layout.chosen_host = None;
        layout.terminals_in_front = false;
        Ok(tab_id)
    })
}

/// Brings the terminals' strip to the front when it has a tab, answering whether it had one.
pub(crate) fn show_terminals(core: &Core) -> CoreResult<bool> {
    edit(core, |layout| {
        let shown = !layout.terminals.tabs.is_empty();
        if shown {
            bring_forward(layout, &At::terminals());
        }
        Ok(shown)
    })
}

/// Brings the tab showing the terminal `id` to the front in the strip `at` names,
/// opening one when none does. Answers its id.
pub(crate) fn show_terminal(
    core: &Core,
    at: At,
    id: SessionId,
    title: Option<String>,
) -> CoreResult<String> {
    open_tab(
        core,
        OpenTab {
            at,
            kind: PaneKind::Terminal,
            target: id.to_string(),
            title,
            preview: false,
            behind: false,
        },
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TabParams {
    #[serde(flatten)]
    at: At,
    tab: String,
    /// Where `moveTab` puts it, counted in the strip as it is now.
    to: Option<usize>,
    /// The name `renameTab` gives it; an empty one gives back its own name.
    title: Option<String>,
}

/// Closes a tab. Agents it shows close with it, since an agent always has a
/// tab; terminals keep running until a client ends them.
pub(crate) fn close_tab(core: &Core, params: TabParams) -> CoreResult<()> {
    close(core, &params.at, &[params.tab])
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TabsParams {
    #[serde(flatten)]
    at: At,
    tabs: Vec<String>,
}

/// Closes several tabs of one strip at once, as `closeTab` closes one.
pub(crate) fn close_tabs(core: &Core, params: TabsParams) -> CoreResult<()> {
    close(core, &params.at, &params.tabs)
}

/// A tab already gone, such as one closed twice before the window heard of the first, is
/// closed already, so closing it does nothing.
fn close(core: &Core, at: &At, tabs: &[String]) -> CoreResult<()> {
    let agents = edit(core, |layout| {
        let Ok(strip) = strip_at(layout, at) else {
            return Ok(Vec::new());
        };
        let closing = |tab: &Tab| tabs.contains(&tab.id);
        if !strip.tabs.iter().any(closing) {
            return Ok(Vec::new());
        }
        let active = strip
            .active
            .as_ref()
            .and_then(|active| strip.tabs.iter().position(|tab| &tab.id == active));
        let mut agents = Vec::new();
        let mut kept_before_active = 0;
        let mut active_closed = false;
        for (at, tab) in std::mem::take(&mut strip.tabs).into_iter().enumerate() {
            if closing(&tab) {
                agents_in(&tab.pane, &mut agents);
                active_closed |= Some(at) == active;
            } else {
                kept_before_active += usize::from(active.is_some_and(|active| at < active));
                strip.tabs.push(tab);
            }
        }
        if active_closed {
            strip.active = strip
                .tabs
                .get(kept_before_active.min(strip.tabs.len().saturating_sub(1)))
                .map(|tab| tab.id.clone());
        }
        Ok(agents)
    })?;
    if !agents.is_empty() {
        super::workspace::close(core, &agents)?;
    }
    Ok(())
}

/// Keeps a tab opened to look at, so the next one opens beside it.
pub(crate) fn keep_tab(core: &Core, params: TabParams) -> CoreResult<()> {
    edit(core, |layout| {
        let strip = strip_at(layout, &params.at)?;
        let tab = strip
            .tabs
            .iter_mut()
            .find(|tab| tab.id == params.tab)
            .ok_or("that tab is not open")?;
        tab.preview = false;
        Ok(())
    })
}

/// Names a tab, or gives it back the name its view gives it.
pub(crate) fn rename_tab(core: &Core, params: TabParams) -> CoreResult<()> {
    edit(core, |layout| {
        let strip = strip_at(layout, &params.at)?;
        let tab = strip
            .tabs
            .iter_mut()
            .find(|tab| tab.id == params.tab)
            .ok_or("that tab is not open")?;
        tab.title = params
            .title
            .map(|title| title.trim().to_owned())
            .filter(|title| !title.is_empty());
        Ok(())
    })
}

/// Opens a copy of a project's tab beside it, with a new shell in place of
/// each terminal it shows. Answers the copy's id.
pub(crate) async fn duplicate_tab(core: &Arc<Core>, params: TabParams) -> CoreResult<String> {
    if params.at.host.is_some() || params.at.terminals {
        return Err("only a project's tabs are copied".into());
    }
    let project = &params.at.project;
    let source = view(core, &core.workspace_view(), &core.session_infos(), None)
        .projects
        .get(project)
        .and_then(|strip| strip.tabs.iter().find(|tab| tab.id == params.tab))
        .cloned()
        .ok_or("that tab is not open")?;
    let mut agents = Vec::new();
    agents_in(&source.pane, &mut agents);
    if !agents.is_empty() {
        return Err("an agent's tab is not copied".into());
    }
    let mut shells = Vec::new();
    for _ in 0..terminals_in(&source.pane) {
        shells.push(super::spare::project_shell(core, project).await?);
    }
    let mut shells = shells.into_iter();
    let copy = Tab {
        id: format!("tab-{}", uuid::Uuid::new_v4().simple()),
        title: source.title.as_ref().map(|title| format!("{title} copy")),
        preview: false,
        pane: copied(&source.pane, &mut shells),
    };
    edit(core, |layout| {
        let strip = strip(layout, project)?;
        if strip.tabs.len() >= MAX_TABS {
            return Err("that project already has as many tabs as the core keeps".into());
        }
        let at = strip
            .tabs
            .iter()
            .position(|tab| tab.id == params.tab)
            .map_or(strip.tabs.len(), |at| at + 1);
        let id = copy.id.clone();
        strip.tabs.insert(at, copy);
        strip.active = Some(id.clone());
        bring_forward(layout, &params.at);
        Ok(id)
    })
}

/// Lays out at the end of `project`'s strip the tabs another host's project at `from` had,
/// carried to this one: its agents swapped for the ones made here from theirs, paths moved
/// under this project's folder, terminals opened afresh, and what holds nothing here left out.
/// The tabs the new agents got of their own give way to these. Answers how many it laid out,
/// and how many still held something here to lay out.
pub(crate) async fn lay_out_carried(
    core: &Arc<Core>,
    project: &str,
    from: &str,
    tabs: Vec<Tab>,
    agents: &HashMap<String, String>,
) -> CoreResult<(usize, usize)> {
    let to = core
        .workspaces
        .project_info(project)
        .ok_or_else(|| CoreError::from("that project is not on this host"))?
        .path
        .to_string_lossy()
        .into_owned();
    let made: HashSet<&str> = agents.values().map(String::as_str).collect();
    let shown = |kind: PaneKind, target: &str| match kind {
        PaneKind::Agent => made.contains(target),
        PaneKind::Desk | PaneKind::Browser | PaneKind::InstallSikemux | PaneKind::Unknown => false,
        _ => true,
    };
    let mut carried = Vec::new();
    for mut tab in tabs {
        moved_into(&mut tab.pane, agents, from, &to);
        if keep_shown(&mut tab.pane, &shown) {
            carried.push(tab);
        }
    }
    let mut shells = Vec::new();
    for _ in 0..carried
        .iter()
        .map(|tab| terminals_in(&tab.pane))
        .sum::<usize>()
    {
        shells.push(super::spare::project_shell(core, project).await?);
    }
    let mut shells = shells.into_iter();
    let wanted = carried.len();
    let carried: Vec<Tab> = carried
        .into_iter()
        .map(|tab| Tab {
            id: format!("tab-{}", uuid::Uuid::new_v4().simple()),
            title: tab.title,
            preview: false,
            pane: copied(&tab.pane, &mut shells),
        })
        .collect();
    edit(core, |layout| {
        let strip = strip(layout, project)?;
        strip.tabs.retain(|tab| {
            !matches!(&tab.pane, Pane::View { kind: PaneKind::Agent, target, .. } if made.contains(target.as_str()))
        });
        let room = MAX_TABS.saturating_sub(strip.tabs.len());
        let laid = wanted.min(room);
        strip.tabs.extend(carried.into_iter().take(room));
        Ok((laid, wanted))
    })
}

/// `pane` pointing at what it showed as this host knows it: an agent by the one made here,
/// a path under `from` by the same path under `to`.
fn moved_into(pane: &mut Pane, agents: &HashMap<String, String>, from: &str, to: &str) {
    match pane {
        Pane::View { kind, target, .. } => match kind {
            PaneKind::Agent => {
                if let Some(made) = agents.get(target.as_str()) {
                    target.clone_from(made);
                }
            }
            PaneKind::Terminal => {}
            _ => {
                if let Some(rest) = target
                    .strip_prefix(from)
                    .filter(|rest| rest.is_empty() || rest.starts_with('/'))
                {
                    *target = format!("{to}{rest}");
                }
            }
        },
        Pane::Split { panes, .. } => {
            for pane in panes {
                moved_into(pane, agents, from, to);
            }
        }
    }
}

fn terminals_in(pane: &Pane) -> usize {
    match pane {
        Pane::View { kind, .. } => usize::from(*kind == PaneKind::Terminal),
        Pane::Split { panes, .. } => panes.iter().map(terminals_in).sum(),
    }
}

/// `pane` with new ids, its terminals swapped in turn for `shells`.
fn copied(pane: &Pane, shells: &mut impl Iterator<Item = SessionId>) -> Pane {
    match pane {
        Pane::View { kind, target, .. } => Pane::View {
            id: view_id(),
            kind: *kind,
            target: match kind {
                PaneKind::Terminal => shells
                    .next()
                    .map_or_else(|| target.clone(), |shell| shell.to_string()),
                _ => target.clone(),
            },
        },
        Pane::Split {
            dir, panes, sizes, ..
        } => Pane::Split {
            id: view_id(),
            dir: *dir,
            panes: panes.iter().map(|pane| copied(pane, shells)).collect(),
            sizes: sizes.clone(),
        },
    }
}

pub(crate) fn activate_tab(core: &Core, params: TabParams) -> CoreResult<()> {
    edit(core, |layout| {
        bring_forward(layout, &params.at);
        let strip = strip_at(layout, &params.at)?;
        if !strip.tabs.iter().any(|tab| tab.id == params.tab) {
            return Err("that tab is not open".into());
        }
        strip.active = Some(params.tab);
        Ok(())
    })
}

pub(crate) fn move_tab(core: &Core, params: TabParams) -> CoreResult<()> {
    edit(core, |layout| {
        let strip = strip_at(layout, &params.at)?;
        let at = strip
            .tabs
            .iter()
            .position(|tab| tab.id == params.tab)
            .ok_or("that tab is not open")?;
        let moved = strip.tabs.remove(at);
        let to = params.to.unwrap_or(strip.tabs.len()).min(strip.tabs.len());
        strip.tabs.insert(to, moved);
        Ok(())
    })
}

#[derive(Deserialize)]
pub(crate) struct ProjectParams {
    project: String,
}

#[derive(Deserialize)]
pub(crate) struct HostParams {
    /// "this", "host:<key>" for another host on the account, or "ssh:<alias>".
    host: String,
}

/// Puts the asking client on a host: this one, another on the account, or an SSH host, whose
/// strip comes forward, opening a terminal there when it has none.
pub(crate) async fn choose_host(core: &Arc<Core>, params: HostParams) -> CoreResult<()> {
    if let Some(alias) = params.host.strip_prefix("ssh:") {
        let has_tabs = core
            .layouts
            .layout
            .locked()
            .hosts
            .get(alias)
            .is_some_and(|strip| !strip.tabs.is_empty());
        if !has_tabs {
            super::spare::open_ssh_terminal(core, super::spare::SshParams::new(alias)).await?;
        }
    }
    edit(core, |layout| {
        layout.terminals_in_front = false;
        match params.host.split_once(':') {
            None if params.host == "this" => {
                layout.chosen_host = None;
                layout.active_host = None;
            }
            Some(("host", key)) => {
                if !core.remote.links().iter().any(|link| link.id == key) {
                    return Err("that host is not linked with this one".into());
                }
                layout.chosen_host = Some(key.to_owned());
                layout.active_host = None;
            }
            Some(("ssh", alias)) => {
                layout.chosen_host = None;
                layout.active_host = Some(alias.to_owned());
            }
            _ => return Err("that is not a host".into()),
        }
        Ok(())
    })
}

#[derive(Deserialize)]
pub(crate) struct OpenPaths {
    targets: Vec<CliOpenTarget>,
    /// Opens the files without bringing their projects forward.
    #[serde(default)]
    behind: bool,
}

/// Opens what `sikemux open` or an agent's `ui_open` names: a folder's project, or a file
/// in its project's editor, in the project holding it, else in the target's project root
/// made a project. Answers each target's project and tab, or why it did not open.
pub(crate) async fn open_paths(core: &Arc<Core>, params: OpenPaths) -> CoreResult<Value> {
    let mut answers = Vec::new();
    for target in params.targets {
        answers.push(match open_path(core, &target, params.behind).await {
            Ok((project, tab)) => json!({ "targetId": target.id, "project": project, "tab": tab }),
            Err(error) => json!({ "targetId": target.id, "error": error.to_string() }),
        });
    }
    Ok(Value::Array(answers))
}

async fn open_path(
    core: &Arc<Core>,
    target: &CliOpenTarget,
    behind: bool,
) -> CoreResult<(String, Option<String>)> {
    let project = owning_project(core, Path::new(&target.path), &target.project_root).await?;
    match target.kind {
        CliTargetKind::Directory => {
            if !behind {
                activate_project(
                    core,
                    ProjectParams {
                        project: project.clone(),
                    },
                )?;
            }
            Ok((project, None))
        }
        CliTargetKind::File => {
            let tab = open_tab(
                core,
                OpenTab {
                    at: At::project(&project),
                    kind: PaneKind::Files,
                    target: target.path.clone(),
                    title: None,
                    preview: false,
                    behind,
                },
            )?;
            Ok((project, Some(tab)))
        }
    }
}

async fn owning_project(core: &Arc<Core>, path: &Path, root: &str) -> CoreResult<String> {
    let projects = core.workspace_view().projects;
    let paths: Vec<_> = projects
        .iter()
        .map(|project| project.path.clone())
        .collect();
    let owner = super::names::project_of(path, &paths);
    let canonical = |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    if let Some(project) = projects
        .iter()
        .find(|project| canonical(&project.path).to_string_lossy() == owner)
    {
        return Ok(project.id.clone());
    }
    let params = super::workspace::AddProjectParams::at(root.into());
    Ok(super::workspace::add_project(core, params).await?.id)
}

pub(crate) fn activate_project(core: &Core, params: ProjectParams) -> CoreResult<()> {
    edit(core, |layout| {
        strip(layout, &params.project)?;
        bring_forward(layout, &At::project(&params.project));
        Ok(())
    })
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Side {
    Left,
    Right,
    Top,
    Bottom,
    /// Behind the view, as one more tab of a stack.
    Stack,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SplitPane {
    #[serde(flatten)]
    at: At,
    tab: String,
    /// The view the new one goes beside, by id.
    pane: String,
    side: Side,
    kind: Option<PaneKind>,
    target: Option<String>,
    /// A tab of the same strip whose pane moves into the split, closing the tab.
    from_tab: Option<String>,
}

/// Puts a view beside one of a tab's views: a new view of `kind` showing
/// `target`, or the pane of `from_tab`. Brings the tab to the front and
/// answers the id of the view it added, or of the first moved one.
pub(crate) fn split_pane(core: &Core, params: SplitPane) -> CoreResult<String> {
    edit(core, |layout| {
        if let (None, Some(PaneKind::Desk), Some(agent)) =
            (&params.from_tab, params.kind, &params.target)
        {
            if let Some((project, tab, view)) = desk_of(layout, agent) {
                if let Some(strip) = layout.projects.get_mut(&project) {
                    strip.active = Some(tab);
                }
                return Ok(view);
            }
        }
        let strip = strip_at(layout, &params.at)?;
        let new = match (&params.from_tab, params.kind, &params.target) {
            (Some(from), _, _) => {
                if from == &params.tab {
                    return Err("a tab cannot be split into itself".into());
                }
                let at = strip
                    .tabs
                    .iter()
                    .position(|tab| &tab.id == from)
                    .ok_or("that tab is not open")?;
                strip.tabs.remove(at).pane
            }
            (None, Some(kind), Some(target)) => Pane::View {
                id: view_id(),
                kind,
                target: target.clone(),
            },
            _ => return Err("a split needs a kind and target, or a tab to move".into()),
        };
        let added = first_view_id(&new);
        let tab = tab_in(strip, &params.tab)?;
        let (dir, after) = match params.side {
            Side::Left => (SplitDir::Row, false),
            Side::Right => (SplitDir::Row, true),
            Side::Top => (SplitDir::Column, false),
            Side::Bottom => (SplitDir::Column, true),
            Side::Stack => (SplitDir::Stack, true),
        };
        let mut new = Some(new);
        if !insert_beside(&mut tab.pane, &params.pane, dir, after, &mut new) {
            return Err("that view is not in the tab".into());
        }
        strip.active = Some(params.tab.clone());
        Ok(added)
    })
}

/// Where the agent's desk already shows, as its project, tab and view: an agent has one desk.
fn desk_of(layout: &Layout, agent: &str) -> Option<(String, String, String)> {
    layout.projects.iter().find_map(|(project, strip)| {
        strip.tabs.iter().find_map(|tab| {
            view_holding(&tab.pane, PaneKind::Desk, agent)
                .map(|view| (project.clone(), tab.id.clone(), view))
        })
    })
}

fn view_holding(pane: &Pane, kind: PaneKind, target: &str) -> Option<String> {
    match pane {
        Pane::View {
            id,
            kind: held,
            target: held_target,
            ..
        } => (*held == kind && held_target == target).then(|| id.clone()),
        Pane::Split { panes, .. } => panes
            .iter()
            .find_map(|pane| view_holding(pane, kind, target)),
    }
}

fn first_view_id(pane: &Pane) -> String {
    match pane {
        Pane::View { id, .. } => id.clone(),
        Pane::Split { panes, .. } => panes.first().map(first_view_id).unwrap_or_default(),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PaneParams {
    #[serde(flatten)]
    at: At,
    tab: String,
    pane: String,
}

/// Closes one view of a tab; a tab left with nothing closes as `closeTab` does.
/// An agent shown in it closes with it. A view already gone is closed already.
pub(crate) fn close_pane(core: &Core, params: PaneParams) -> CoreResult<()> {
    let closed = edit(core, |layout| {
        let Some(tab) = strip_at(layout, &params.at)
            .ok()
            .and_then(|strip| tab_in(strip, &params.tab).ok())
        else {
            return Ok(None);
        };
        let Some(view) = find_view(&tab.pane, &params.pane).cloned() else {
            return Ok(None);
        };
        let keep = |pane: &Pane| !matches!(pane, Pane::View { id, .. } if id == &params.pane);
        let left = keep(&tab.pane) && prune(&mut tab.pane, &keep);
        Ok(Some((view, left)))
    })?;
    match closed {
        None => Ok(()),
        Some((_, false)) => close_tab(
            core,
            TabParams {
                at: params.at,
                tab: params.tab,
                to: None,
                title: None,
            },
        ),
        Some((view, true)) => {
            let mut agents = Vec::new();
            agents_in(&view, &mut agents);
            if !agents.is_empty() {
                super::workspace::close(core, &agents)?;
            }
            Ok(())
        }
    }
}

/// Takes one view of a split tab out into a tab of its own, beside the tab it was in, and
/// brings that one to the front. Answers the new tab's id.
pub(crate) fn separate_pane(core: &Core, params: PaneParams) -> CoreResult<String> {
    edit(core, |layout| {
        let strip = strip_at(layout, &params.at)?;
        if strip.tabs.len() >= MAX_TABS {
            return Err("that strip already has as many tabs as the core keeps".into());
        }
        let at = strip
            .tabs
            .iter()
            .position(|tab| tab.id == params.tab)
            .ok_or("that tab is not open")?;
        let tab = &mut strip.tabs[at];
        if matches!(tab.pane, Pane::View { .. }) {
            return Err("that tab shows only that view".into());
        }
        let view = find_view(&tab.pane, &params.pane)
            .cloned()
            .ok_or("that view is not in the tab")?;
        let keep = |pane: &Pane| !matches!(pane, Pane::View { id, .. } if id == &params.pane);
        prune(&mut tab.pane, &keep);
        let id = format!("tab-{}", uuid::Uuid::new_v4().simple());
        strip.tabs.insert(
            at + 1,
            Tab {
                id: id.clone(),
                title: None,
                pane: view,
                preview: false,
            },
        );
        strip.active = Some(id.clone());
        Ok(id)
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResizeSplit {
    #[serde(flatten)]
    at: At,
    tab: String,
    split: String,
    /// Each pane's share, in any unit; the core keeps them in thousandths.
    sizes: Vec<u16>,
}

pub(crate) fn resize_split(core: &Core, params: ResizeSplit) -> CoreResult<()> {
    edit(core, |layout| {
        let strip = strip_at(layout, &params.at)?;
        let tab = tab_in(strip, &params.tab)?;
        let Some(Pane::Split { panes, sizes, .. }) = find_split(&mut tab.pane, &params.split)
        else {
            return Err("that split is not in the tab".into());
        };
        if params.sizes.len() != panes.len() {
            return Err("a split needs one size for each of its panes".into());
        }
        *sizes = shares(&params.sizes);
        Ok(())
    })
}
