//! Delivering a stashed item into a chat's draft, as the account says one arrived.

use super::*;

/// The items being put in a chat's draft right now, so an event and a catch-up that both
/// hear of one deliver it once.
pub(super) struct Delivering(String);

impl Delivering {
    pub(super) fn held() -> &'static Mutex<HashSet<String>> {
        static HELD: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
        HELD.get_or_init(Default::default)
    }

    pub(super) fn start(id: &str) -> Option<Self> {
        Self::held()
            .locked()
            .insert(id.to_owned())
            .then(|| Self(id.to_owned()))
    }
}

impl Drop for Delivering {
    fn drop(&mut self) {
        Self::held().locked().remove(&self.0);
    }
}

/// What the live connection heard about the stash: every app reads it again, and an item
/// shared to one of this host's chats is delivered there.
pub(crate) fn heard(core: &Arc<Core>, events: &[AccountEvent]) {
    let ids: Vec<String> = events
        .iter()
        .filter(|event| {
            matches!(
                event.r#type,
                AccountEventType::StashAdded | AccountEventType::StashChanged
            )
        })
        .filter_map(|event| event.stash_id.clone())
        .collect();
    let stash_moved = events.iter().any(|event| {
        matches!(
            event.r#type,
            AccountEventType::StashAdded
                | AccountEventType::StashChanged
                | AccountEventType::StashRemoved
                | AccountEventType::StashSeen
        )
    });
    if stash_moved {
        core.changed(What::Read(Topic::Stash));
    }
    if ids.is_empty() {
        return;
    }
    let core = core.clone();
    tokio::spawn(async move {
        for id in ids {
            match item(&core, &id).await {
                Ok(item) => deliver(&core, item).await,
                Err(error) => eprintln!("sikemux core: could not read stash item {id}: {error}"),
            }
        }
    });
}

/// Delivers every item shared to this host's chats while it was away, once its live
/// connection is up again.
pub(crate) fn catch_up(core: &Arc<Core>) {
    let core = core.clone();
    tokio::spawn(async move {
        let mut cursor = None;
        loop {
            let list = match page(&core, StashTaken::False, cursor.as_deref()).await {
                Ok(list) => list,
                Err(error) => {
                    eprintln!("sikemux core: could not read the stash: {error}");
                    return;
                }
            };
            for item in list.items {
                deliver(&core, item).await;
            }
            match list.cursor {
                Some(next) => cursor = Some(next),
                None => return,
            }
        }
    });
}

/// Puts `item` in the chat it was shared to, when that chat is on this host, and marks it
/// used. One shared to a chat since closed, or a project no longer open here, stays in the
/// stash for the person to attach.
pub(super) async fn deliver(core: &Arc<Core>, item: StashItem) {
    if item.taken_at.is_some() {
        return;
    }
    let Some(target) = item.target.as_ref() else {
        return;
    };
    if core.remote.core_id().as_deref() != Some(target.host_key.as_str()) {
        return;
    }
    let Some(_delivering) = Delivering::start(&item.id) else {
        return;
    };
    if let Err(error) = deliver_to(core, &item, target).await {
        eprintln!(
            "sikemux core: could not put stash item {} in its chat: {error}",
            item.id
        );
    }
}

pub(super) async fn deliver_to(
    core: &Arc<Core>,
    item: &StashItem,
    target: &StashTarget,
) -> CoreResult<()> {
    let agent_id = match (&target.chat_id, &target.project_path) {
        (Some(chat), _) if core.workspaces.agent(chat).is_some() => chat.clone(),
        (Some(_), _) => return Ok(()),
        (None, Some(project)) if target.new_chat == Some(true) => {
            match new_chat(core, project).await? {
                Some(agent_id) => agent_id,
                None => return Ok(()),
            }
        }
        _ => return Ok(()),
    };
    into_draft(core, item, &agent_id).await?;
    take(core, &item.id).await?;
    core.changed(What::Read(Topic::Stash));
    Ok(())
}

/// Opens a new chat in the project at `folder`, as a new chat there opens: it waits for the
/// person, so its agent starts only once they write or send. None when that project is not
/// open on this host or no agent here can chat.
pub(super) async fn new_chat(core: &Arc<Core>, folder: &str) -> CoreResult<Option<String>> {
    let view = core.workspace_view();
    let Some(project) = view
        .projects
        .iter()
        .find(|project| project.path.to_string_lossy() == folder)
    else {
        return Ok(None);
    };
    let Some(launcher) = project.launcher.clone() else {
        return Ok(None);
    };
    let agent_id = crate::server::names::new_agent_id();
    let chosen = super::super::launching::Launch {
        agent_id: agent_id.clone(),
        launcher,
        cwd: project.path.clone(),
        permission_mode: None,
        model: None,
        model_provider: None,
        effort: None,
        resume_id: None,
        account: None,
        fallbacks: Vec::new(),
        program: None,
        waits_for_person: true,
    };
    super::super::launching::start(core, None, chosen).await?;
    let tab = json!({ "project": project.id, "kind": "agent", "target": agent_id });
    if let Ok(tab) = serde_json::from_value(tab) {
        let _ = super::super::layout::open_tab(core, tab);
    }
    Ok(Some(agent_id))
}

#[cfg(test)]
mod contract {
    use super::super::{MAX_ITEM_BYTES, MAX_PROMPT_CHARS};

    /// What every sender of a stash upload agrees on, read by each language's tests.
    const VECTOR: &str = include_str!("../../../../../server/protocol/vectors/stash.json");

    #[test]
    fn the_limits_are_the_servers() {
        let vector: serde_json::Value = serde_json::from_str(VECTOR).expect("the vector parses");
        assert_eq!(vector["maxFileBytes"], MAX_ITEM_BYTES);
        assert_eq!(vector["maxText"], MAX_PROMPT_CHARS);
        assert_eq!(vector["headers"]["name"], "x-stash-name");
        assert_eq!(vector["headers"]["prompt"], "x-stash-prompt");
    }
}
