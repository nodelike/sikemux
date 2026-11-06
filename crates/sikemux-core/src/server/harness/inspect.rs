//! What `workspace_inspect` answers: the project as the core knows it, with the panes the window shows.

use super::*;

pub(super) async fn inspect(core: &Arc<Core>, request: HarnessRequest) -> Result<Value, String> {
    let project = request.project.clone();
    let agent_id = request.agent_id.clone();
    let window_open = core.window.is_open();
    let layout = if window_open {
        forward(core, request).await.ok()
    } else {
        None
    }
    .or_else(|| laid_out(core, &project));
    let mut value = {
        let (core, project) = (core.clone(), project.clone());
        blocking(move || Ok(super::super::tasks::inspect(&core, Path::new(&project))))
            .await
            .map_err(|error| error.to_string())?
    };
    if let Some(object) = value.as_object_mut() {
        object.insert("project".into(), project.clone().into());
        object.insert("agentId".into(), agent_id.into());
        if let Some(Value::Object(mut layout)) = layout {
            for key in LAYOUT_FIELDS {
                if let Some(value) = layout.remove(key) {
                    object.insert(key.into(), value);
                }
            }
        }
        if !window_open {
            object.insert("note".into(), NO_WINDOW_NOTE.into());
        }
        core.harness.read(|state| {
            object.insert(
                "runs".into(),
                state
                    .runs
                    .list(&project)
                    .iter()
                    .map(for_agent)
                    .collect::<Vec<_>>()
                    .into(),
            );
            object.insert(
                "cursor".into(),
                state.journals.cursor(&project).to_string().into(),
            );
        });
    }
    Ok(value)
}

/// The project's tabs and the views in them as the core keeps them, for a window that
/// cannot list its own panes.
pub(super) fn laid_out(core: &Core, project: &str) -> Option<Value> {
    let workspace = core.workspace_view();
    let id = workspace
        .projects
        .iter()
        .find(|known| known.path == Path::new(project))?
        .id
        .clone();
    let (_, sessions) = core.ranked_infos();
    let layout = super::super::layout::view(core, &workspace, &sessions, None);
    let strip = layout.projects.get(&id)?;
    let windows: Vec<Value> = strip
        .tabs
        .iter()
        .map(|tab| {
            let mut panes = Vec::new();
            views(core, &tab.pane, &mut panes);
            json!({ "id": tab.id, "name": tab.title, "panes": panes })
        })
        .collect();
    Some(json!({
        "sessionId": id,
        "active": layout.active_project.as_deref() == Some(id.as_str()),
        "activeWindowId": strip.active,
        "windows": windows,
    }))
}

pub(super) fn views(core: &Core, pane: &Pane, found: &mut Vec<Value>) {
    match pane {
        Pane::View { id, kind, target } => {
            let mut view = json!({ "id": id, "kind": kind, "target": target });
            if let Some(cwd) = cwd_of(core, *kind, target) {
                view["cwd"] = cwd.to_string_lossy().into_owned().into();
            }
            found.push(view)
        }
        Pane::Split { panes, .. } => panes.iter().for_each(|pane| views(core, pane, found)),
    }
}

/// The folder a terminal or agent pane works in: a chat's launch folder, else its terminal's.
pub(super) fn cwd_of(core: &Core, kind: PaneKind, target: &str) -> Option<PathBuf> {
    match kind {
        PaneKind::Terminal => core.session(target.parse().ok()?)?.cwd(),
        PaneKind::Agent => match core.chats.get(target) {
            Some(chat) => Some(chat.launch.cwd.clone()),
            None => core.agent_session(target)?.cwd(),
        },
        _ => None,
    }
}
