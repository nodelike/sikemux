//! The core's methods a client reaches by name through `Request::Call`. A
//! device gains a method here without its native code changing, so one a
//! released app calls is never removed or renamed.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::access::{Needs, Peer};
use super::attachments;
use super::changed::Topic;
use super::connection::{blocking, ClientConn};
use super::edits::{self, Edit, Filter, Owners};
use super::names::{AgentName, ClosedAgent, NameTheme};
use super::workspace;
use super::{
    agent_accounts, agent_sign_in, board, inbox, messages, model_providers, plugins, questions,
    remote, saved, saved_sessions, share, skill_library, who_wrote, Core, CoreError, CoreResult,
};
use crate::protocol::{AgentBuddy, Preferences, RemoteStatus};
use sikemux_wire::catalog::CatalogAgent;
use sikemux_wire::chat::ToolDetail;
use sikemux_wire::questions::AgentQuestion;

/// Each method once: what a client must be allowed, `any_program` when any
/// program on this host may call it rather than only Sikemux's own window or
/// notch, and what it runs. A method whose params and answer are named, as
/// `"m" Marker (Params) -> Answer`, runs on its params read as that type and answers that
/// type, which clients are generated to call it with, and is `call::Marker` to a
/// client in Rust.
macro_rules! calls {
    ($core:ident, $client:ident, $params:ident;
     $($name:literal $($marker:ident ($takes:ty) -> $gives:ty)?: $needs:ident $(, $any:ident)? => $run:expr,)*) => {
        /// The methods that name their params and answer, one type each, for a client
        /// in Rust to call them typed.
        pub mod call {
            pub trait Call {
                const METHOD: &'static str;
                type Params;
                type Answer;
            }

            $($(pub struct $marker;)?)*
        }

        $($(
            impl call::Call for call::$marker {
                const METHOD: &'static str = $name;
                type Params = $takes;
                type Answer = $gives;
            }
        )?)*

        fn rule(method: &str) -> Option<(Needs, bool)> {
            match method {
                $($name => Some((Needs::$needs, calls!(@any $($any)?))),)*
                _ => None,
            }
        }

        pub(crate) async fn call(
            $core: Arc<Core>,
            $client: &ClientConn,
            method: &str,
            $params: Value,
        ) -> CoreResult<Value> {
            match method {
                $($name => calls!(@run $params, $run $(, $takes, $gives)?),)*
                method => match Routed::of(method) {
                    Some(Routed::Git) => super::git::call(&$core, &$client.peer, method, $params).await,
                    Some(Routed::Files) => files(&$core, method, $params).await,
                    Some(Routed::Lsp) => super::lsp::call(&$core, $client.id, method, $params).await,
                    None => Err(format!("this host does not know {method}").into()),
                },
            }
        }
    };
    (@any any_program) => { true };
    (@any) => { false };
    (@run $params:ident, $run:expr) => { $run };
    (@run $params:ident, $run:expr, $takes:ty, $gives:ty) => {{
        #[allow(unused_variables)]
        let $params: $takes = parse($params)?;
        let gives: $gives = $run;
        answer(gives)
    }};
}

/// The methods other modules route among themselves, known by their name's prefix.
#[derive(Clone, Copy)]
enum Routed {
    Git,
    Files,
    Lsp,
}

impl Routed {
    fn of(method: &str) -> Option<Self> {
        let (namespace, _) = method.split_once('.')?;
        match namespace {
            "git" => Some(Routed::Git),
            "files" => Some(Routed::Files),
            "lsp" => Some(Routed::Lsp),
            _ => None,
        }
    }
}

pub(crate) fn needs(method: &str) -> Needs {
    match (rule(method), Routed::of(method)) {
        (Some((needs, _)), _) => needs,
        (None, Some(Routed::Git)) => super::git::needs(method),
        (None, Some(Routed::Files | Routed::Lsp)) => Needs::Full,
        (None, None) => Needs::Local,
    }
}

/// Whether only Sikemux's own window or notch may call `method` from this
/// host; another program here may only read, or start a plugin's browser
/// sign-in, which only the person can finish in their browser.
pub(crate) fn for_person(method: &str) -> bool {
    match (rule(method), Routed::of(method)) {
        (Some((_, any_program)), _) => !any_program,
        (None, Some(Routed::Git)) => super::git::for_person(method),
        (None, Some(Routed::Files | Routed::Lsp) | None) => true,
    }
}

calls! { core, client, params;
    "chatFile": Watch, any_program => answer(chat_file(&core, &client.peer, parse(params)?).await?),
    "keepPasted": Full => answer(keep_pasted(&core, parse(params)?).await?),
    "page": Watch, any_program => answer(page(&core, parse(params)?).await?),
    "nameAgent": Local => answer(name_agent(&core, parse(params)?).await?),
    "renameAgent": Full => answer(rename_agent(&core, parse(params)?).await?),
    "promptChat" PromptChat (PromptChatParams) -> (): Full => prompt_chat(&core, params).await?,
    "closedAgents" ClosedAgents (ClosedAgentsParams) -> Vec<super::names::ClosedAgent>: Full => {
        closed_agents(&core, params).await?
    },
    "agentNames": Watch, any_program => answer(agent_names(&core).await?),
    "agentCatalog" AgentCatalog (NoParams) -> Vec<CatalogAgent>: Watch, any_program => super::catalog::view(),
    "agentNameTheme": Full => answer(NameThemeParams { theme: core.names.theme() }),
    "setAgentNameTheme": Person => {
        let params: NameThemeParams = parse(params)?;
        core.names.set_theme(params.theme);
        answer(params)
    },
    "hostLoad": Watch, any_program => answer(super::load::now()),
    "stopChatIfIdle": Local => {
        let params: AgentParams = parse(params)?;
        answer(json!({ "stopped": super::lifecycle::sleep(&core, &params.agent_id) }))
    },
    "switchAgentView": Person => answer(super::agent_view::switch(&core, parse(params)?).await?),
    "sleepAgent": Person => {
        let params: AgentParams = parse(params)?;
        answer(super::lifecycle::sleep_agent(&core, &params.agent_id)?)
    },
    "sleepIdleChats": Local => answer(json!({ "slept": super::lifecycle::sleep_idle(&core) })),
    "startTerminalAgent": Local => {
        let params: TerminalParams = parse(params)?;
        let size = (params.cols, params.rows);
        let pty = super::launching::start_terminal(&core, &params.agent_id, size, None).await?;
        answer(json!({ "ptyId": pty }))
    },
    "commandUsed": Local => {
        let used: CommandUsed = parse(params)?;
        core.activity_log.command_used(&used.id);
        answer(())
    },
    "composing": Person => {
        let params: AgentParams = parse(params)?;
        super::chat::composing(&core, &params.agent_id);
        answer(())
    },
    "startChat": Local => {
        let params: StartParams = parse(params)?;
        answer(super::launching::start_saved(&core, &params.agent_id, params.waits_for_person).await?)
    },
    "resumeSession": Full => answer(super::launching::resume_session(&core, parse(params)?).await?),
    "activitySummary": Watch, any_program => answer(activity_summary(&core, parse(params)?).await?),
    "setActivityPrice": Person => {
        let params: ActivityPriceParams = parse(params)?;
        core.activity_log.set_price(&params.model, params.price)?;
        core.changes.changed(Topic::Activity, None, None);
        answer(())
    },
    "steerQueued" SteerQueued (QueuedParams) -> (): Full => {
        super::chat::queue::steer(&core, &params.agent_id, params.ids, params.message).await?
    },
    "editMessage" EditMessage (EditParams) -> (): Full => edit_message(&core, params).await?,
    "chatPage": Watch, any_program => {
        let params: ChatPageParams = parse(params)?;
        let reading = core.clone();
        answer(blocking(move || super::chat::page(&reading, params)).await?)
    },
    "chatFind" ChatFind (super::chat::find::FindParams) -> super::chat::find::FindAnswer: Watch, any_program => {
        let (reading, client) = (core.clone(), client.id);
        blocking(move || super::chat::find(&reading, client, params)).await?
    },
    "chatToolDetail" ChatToolDetail (ToolDetailParams) -> ToolDetail: Watch, any_program => {
        super::chat::tool_detail(&core, &params.agent_id, &params.tool_call_id)?
    },
    "dropQueued" DropQueued (QueuedParams) -> (): Full => {
        super::chat::queue::remove(&core, &params.agent_id, &params.ids)?
    },
    "agentSessions" AgentSessions (saved::SearchParams) -> Vec<crate::sessions::search::SessionMatch>: Full, any_program => {
        saved::call(&core, params).await?
    },
    "workspace": Watch, any_program => answer(core.workspace_view()),
    "runCommand": Local => answer(super::custom_command::run(parse(params)?).await?),
    "updateStatus": Watch, any_program => answer(core.updates.status()),
    "updateCheck": Person => answer(super::updates::check(&core).await?),
    "updateInstall": Person => answer(super::updates::install(&core, parse(params)?)?),
    "updateCancel": Person => answer(super::updates::cancel(&core)),
    "updateHost": Local, any_program => answer(super::updates::update_host(&core).await?),
    "setHostUpdates": Person => answer(super::updates::set_automatic(&core, parse(params)?).await?),
    "runCustomCommand": Person => answer(super::custom_command::run_custom(&core, parse(params)?).await?),
    "addProject": Person => answer(workspace::add_project(&core, parse(params)?).await?),
    "removeProject": Person => answer(workspace::remove_project(&core, parse(params)?)?),
    "updateProject": Person => answer(workspace::update_project(&core, parse(params)?)?),
    "moveProject": Person => answer(workspace::move_project(&core, parse(params)?)?),
    "setSpaces": Person => answer(workspace::set_spaces(&core, parse(params)?)?),
    "setPinnedChats": Person => answer(workspace::set_pinned_chats(&core, parse(params)?)?),
    "pinChat": Person => answer(workspace::pin_chat(&core, parse(params)?)?),
    "recentChats": Person => answer(workspace::recent_chats(&core, parse(params)?).await?),
    "setAgentDefaults": Person => answer(workspace::set_defaults(&core, parse(params)?).await?),
    "createAgent": Person => answer(workspace::create_agent(&core, parse(params)?)?),
    "updateAgent": Person => answer(workspace::update_agent(&core, parse(params)?)?),
    "moveAgentToProject": Person => answer(super::move_agent::move_agent(&core, parse(params)?).await?),
    "moveAgentToFolder": Person => answer(super::move_agent::move_to_folder(&core, parse(params)?).await?),
    "closeAgents": Person => answer(workspace::close_agents(&core, parse(params)?)?),
    "closeChat" CloseChat (AgentParams) -> (): Full => workspace::close_agent(&core, params.agent_id)?,
    "createAgentWorktree" CreateAgentWorktree (super::worktree::agent::ForAgentParams) -> super::worktree::agent::ForAgent: Person => {
        super::worktree::agent::for_agent(&core, params).await?
    },
    "agentReview": Watch => answer(super::worktree::removal::review(&core, parse(params)?).await?),
    "worktreeMerged": Watch => answer(super::worktree::removal::merged(&core, parse(params)?).await?),
    "worktreeRemoval": Person => answer(super::worktree::removal::removal(&core, parse(params)?).await?),
    "removeAgentWorktree": Person => answer(super::worktree::removal::remove(&core, parse(params)?).await?),
    "agentWorktreeAt": Full => answer(super::worktree::agent::at(parse(params)?).await?),
    "savedSessions": Full, any_program => answer(saved_sessions::list(parse(params)?).await?),
    "recentSessions": Full, any_program => answer(saved_sessions::recent(parse(params)?).await?),
    "renameSession": Person => answer(saved_sessions::rename(parse(params)?).await?),
    "deleteSession": Person => answer(saved_sessions::delete(parse(params)?).await?),
    "renameRecent": Person => answer(saved_sessions::rename_recent(&core, parse(params)?).await?),
    "deleteRecent": Person => answer(saved_sessions::delete_recent(&core, parse(params)?).await?),
    "relocateSession": Person => answer(saved_sessions::relocate(parse(params)?).await?),
    "skillsOverview": Full, any_program => skill_library::overview(&core).await,
    "skillsAdded": Full, any_program => skill_library::added(&core).await,
    "createSkill": Person => skill_library::create(&core, parse(params)?).await,
    "installSkill": Person => skill_library::install(&core, parse(params)?).await,
    "removeSkill": Person => skill_library::remove(&core, parse(params)?).await,
    "adoptSkill": Person => skill_library::adopt(&core, parse(params)?).await,
    "agentSignIns": Full, any_program => answer(agent_sign_in::pages()),
    "agentSignIn": Person => answer(agent_sign_in::sign_in(&core, parse(params)?).await?),
    "signInLauncher": Person => answer(agent_sign_in::sign_in_launcher(&core, parse(params)?).await?),
    "agentSignInCode": Person => answer(agent_sign_in::sign_in_code(parse(params)?).await?),
    "cancelAgentSignIn": Person => {
        agent_sign_in::cancel(parse(params)?);
        answer(())
    },
    "agentSignOut": Person => answer(agent_sign_in::sign_out(parse(params)?).await?),
    "modelProviders": Full, any_program => answer(model_providers::list(&core).await?),
    "modelProviderModels": Full, any_program => {
        answer(model_providers::models(&core, parse(params)?).await?)
    },
    "modelHelper": Full, any_program => answer(model_providers::helper(&core).await?),
    "modelProviderCheck": Person => answer(model_providers::check(&core, parse(params)?).await?),
    "modelProviderConnect": Person => {
        answer(model_providers::connect(&core, parse(params)?).await?)
    },
    "modelProviderDisconnect": Person => {
        answer(model_providers::disconnect(&core, parse(params)?).await?)
    },
    "modelProviderRead": Person => answer(model_providers::read(&core, parse(params)?).await?),
    "modelProviderAdd": Person => answer(model_providers::add(&core, parse(params)?).await?),
    "modelProviderRemove": Person => {
        answer(model_providers::remove(&core, parse(params)?).await?)
    },
    "setModelHelper": Person => answer(model_providers::set_helper(&core, parse(params)?).await?),
    "agentAccounts": Full, any_program => answer(core.workspaces.accounts()),
    "agentStatus": Full, any_program => answer(agent_accounts::status(&core, parse(params)?).await?),
    "installedAgents": Full, any_program => answer(agent_accounts::installed(&core).await?),
    "agentSetup": Full, any_program => answer(agent_accounts::setup(&core).await?),
    "agentAvailability": Full, any_program => {
        answer(agent_accounts::availability(&core, parse(params)?).await?)
    },
    "agentAccountStatus": Full, any_program => {
        answer(agent_accounts::account_status(&core, parse(params)?).await?)
    },
    "agentUsage": Full, any_program => answer(agent_accounts::usage(&core, parse(params)?).await?),
    "agentUsages": Watch, any_program => answer(agent_accounts::usages(&core).await),
    "readAgentUsages": Full, any_program => answer(agent_accounts::read_usages(&core).await),
    "openTab": Full => answer(super::layout::open_tab(&core, parse(params)?)?),
    "keepTab": Full => answer(super::layout::keep_tab(&core, parse(params)?)?),
    "closeTab": Full => answer(super::layout::close_tab(&core, parse(params)?)?),
    "activateTab": Full => answer(super::layout::activate_tab(&core, parse(params)?)?),
    "moveTab": Full => answer(super::layout::move_tab(&core, parse(params)?)?),
    "closeTabs": Full => answer(super::layout::close_tabs(&core, parse(params)?)?),
    "renameTab": Full => answer(super::layout::rename_tab(&core, parse(params)?)?),
    "duplicateTab": Full => answer(super::layout::duplicate_tab(&core, parse(params)?).await?),
    "activateProject": Full => answer(super::layout::activate_project(&core, parse(params)?)?),
    "openPaths": Person => answer(super::layout::open_paths(&core, parse(params)?).await?),
    "chooseHost": Full => answer(super::layout::choose_host(&core, parse(params)?).await?),
    "splitPane": Full => answer(super::layout::split_pane(&core, parse(params)?)?),
    "closePane": Full => answer(super::layout::close_pane(&core, parse(params)?)?),
    "separatePane": Full => answer(super::layout::separate_pane(&core, parse(params)?)?),
    "resizeSplit": Full => answer(super::layout::resize_split(&core, parse(params)?)?),
    "account": Watch => answer(core.account.shown()),
    "remoteImage": Watch => answer(super::remote_image::fetch(&core.remote_images, parse(params)?).await?),
    "accountPicture": Watch => {
        let params: AccountPictureParams = parse(params)?;
        answer(super::account::picture(&core, &params.hash).await)
    },
    "accountSignIn": Person => answer(super::account::sign_in(&core, client.peer.clone()).await?),
    "accountSignInFinish": Person => answer(super::account::sign_in_finished(&core).await?),
    "accountSignInCancel": Person => {
        super::account::cancel_sign_in(&core);
        answer(Value::Null)
    },
    "accountSignOut": Person => answer(super::account::sign_out(&core, &client.peer).await?),
    "sshHosts": Watch, any_program => {
        let core = core.clone();
        answer(blocking(move || Ok(super::ssh::hosts(&core))).await?)
    },
    "removeTrigger": Full => {
        let params: TriggerParams = parse(params)?;
        answer(super::triggers::remove(&core, params.trigger))
    },
    "cancelResume" CancelResume (agent_accounts::Resuming) -> (): Full => agent_accounts::cancel_resume(&core, params),
    "setAgentAccounts": Person => {
        agent_accounts::set(&core, parse(params)?).await?;
        answer(())
    },
    "addAgentAccount": Person => answer(agent_accounts::add(parse(params)?).await?),
    "forgetAgentStatus": Person => {
        agent_accounts::forget(&core, parse(params)?);
        answer(())
    },
    "markAgentSignedOut": Person => {
        agent_accounts::mark_signed_out(&core, parse(params)?);
        answer(())
    },
    "saveAgentAccount": Local => answer(agent_accounts::save(&core, parse(params)?).await?),
    "removeAgentAccount": Local => answer(agent_accounts::remove(&core, parse(params)?).await?),
    "selectAgentAccount": Local => answer(agent_accounts::select(&core, parse(params)?).await?),
    "setAccountAutoSwitch": Local => {
        answer(agent_accounts::set_auto_switch(&core, parse(params)?).await?)
    },
    "retryFailed" RetryFailed (agent_accounts::Resuming) -> (): Full => agent_accounts::retry_failed(&core, params)?,
    "discardFailed" DiscardFailed (agent_accounts::Resuming) -> (): Full => agent_accounts::discard_failed(&core, params)?,
    "chatAccounts" ChatAccounts (agent_accounts::Resuming) -> agent_accounts::ChatAccounts: Full => {
        agent_accounts::chat_accounts(&core, params)?
    },
    "moveChatAccount" MoveChatAccount (agent_accounts::Moving) -> (): Full => agent_accounts::move_chat(&core, params)?,
    "authenticateChat": Local => {
        agent_accounts::authenticate(&core, parse(params)?).await?;
        answer(())
    },
    "shareChat": Person => {
        let params: ShareChatParams = parse(params)?;
        answer(share::share_chat(
            &core,
            &params.agent_id,
            params.lifetime_ms,
            params.title,
        )?)
    },
    "stopShare": Person => {
        let params: StopShareParams = parse(params)?;
        answer(share::stop(&core, &params.id)?)
    },
    "shares": Full, any_program => answer(share::list(&core)),
    "pairing" Pairing (NoParams) -> remote::Pairing: Person, any_program => remote::pairing(&core),
    "deviceChallenge": Person => answer(super::hosts::account::device_challenge(&core).await?),
    "approveHostSignIn": Person => {
        answer(super::hosts::account::approve_host_sign_in(&core, parse(params)?).await?)
    },
    "deviceJoinTicket": Person => {
        answer(super::hosts::account::device_join_ticket(&core, parse(params)?).await?)
    },
    "signHostRequest": Local => {
        let request: crate::accounts::HostRequest = parse(params)?;
        answer(signed(core.remote.sign_host_request(&request)?))
    },
    "signShareViewer": Local => {
        let params: NonceParams = parse(params)?;
        answer(signed(core.remote.sign_share_viewer(&params.nonce)?))
    },
    "answerPairing" AnswerPairing (remote::PairingAnswer) -> remote::Pairing: Person => remote::answer_pairing(&core, params)?,
    "changeDevice" ChangeDevice (remote::DeviceChange) -> remote::Pairing: Person => remote::change_device(&core, params)?,
    "projectFiles": Full => {
        let params = parse(params)?;
        answer(blocking(move || super::project_files::read(params).map_err(CoreError::from)).await?)
    },
    "projectEntries" ProjectEntries (super::project_files::ProjectEntriesParams) -> Vec<crate::fuzzy::Entry>: Full => {
        blocking(move || super::project_files::entries(params).map_err(CoreError::from)).await?
    },
    "searchStream": Full => answer(search_stream(&core, client, parse(params)?)?),
    "searchReplace": Full => {
        let params = parse(params)?;
        answer(blocking(move || super::search::replace(params).map_err(CoreError::from)).await?)
    },
    "fileWindow": Full => {
        let params = parse(params)?;
        answer(blocking(move || super::search::window(params).map_err(CoreError::from)).await?)
    },
    "streamCancel": Watch => {
        let params: StreamParams = parse(params)?;
        core.streams.cancel(client.id, params.stream);
        answer(Value::Null)
    },
    "projectRoots": Full => {
        let params = parse(params)?;
        answer(blocking(move || super::project_roots::scan(params).map_err(CoreError::from)).await?)
    },
    "resolvePath": Full => {
        let params = parse(params)?;
        answer(blocking(move || Ok(super::project_roots::resolve(params))).await?)
    },
    "sshConfigFile": Full => answer(blocking(|| super::ssh::config_file().map_err(CoreError::from)).await?),
    "faceStyle": Watch => answer(core.faces.style()),
    "setFaceStyle": Person => {
        let style = core.faces.set_style(parse(params)?)?;
        core.view_changed();
        answer(style)
    },
    "preferences" Preferences (NoParams) -> Preferences: Watch => core.preferences.get(),
    "stash": Person => {
        let params = serde_json::from_value(params).unwrap_or_default();
        answer(super::stash::list(&core, params).await?)
    },
    "stashPrompt" StashPrompt (super::stash::PromptParams) -> super::stash::Stashed: Person => {
        super::stash::stash_prompt(&core, params).await?
    },
    "seeStash": Person => answer(super::stash::see(&core, parse(params)?).await?),
    "stashFile": Person => answer(super::stash::keep(&core, parse(params)?).await?),
    "stashPicture": Person => answer(super::stash::picture(&core, parse(params)?).await?),
    "attachStash" AttachStash (super::stash::AttachParams) -> super::stash::Attached: Person => {
        super::stash::attach(&core, params).await?
    },
    "removeStash" RemoveStash (super::stash::ItemParams) -> (): Person => super::stash::remove(&core, params).await?,
    "drafts": Watch => {
        let params: DraftsParams = serde_json::from_value(params).unwrap_or_default();
        answer(core.drafts.get(params.keys.as_deref()))
    },
    "setDraft" SetDraft (SetDraftParams) -> (): Person => {
        core.drafts.save(&params.key, params.text, params.extra, params.rev)?;
        super::drafts::changed(&core, vec![params.key]);
    },
    "diagramLayout": Person => {
        let params: DiagramParams = parse(params)?;
        answer(json!({ "layout": core.diagrams.get(&params.key) }))
    },
    "setDiagramLayout": Person => {
        let params: DiagramParams = parse(params)?;
        core.diagrams.set(&params.key, params.layout)?;
        answer(json!({}))
    },
    "setPreference": Person => {
        let params: SetPreferenceParams = parse(params)?;
        core.preferences.set(&params.key, params.value)?;
        super::about::introduced(&core, &params.key)?;
        if params.key == "notch" {
            super::notch::follow(&core);
        }
        if params.key == "updateChannel" {
            core.updates.check_soon();
        }
        if matches!(params.key.as_str(), "paneShader" | "paneImage") {
            super::backdrop::follow(&core);
        }
        if matches!(params.key.as_str(), "agentWorktreeDefaults" | "lastAgentType") {
            core.view_changed();
        }
        let preferences = core.preferences.get();
        core.theme_changed();
        core.changes.changed(Topic::Preferences, None, None);
        answer(preferences)
    },
    "setWorktreeDefault" SetWorktreeDefault (WorktreeDefaultParams) -> (): Person => {
        let mut defaults = core.preferences.get().agent_worktree_defaults;
        match params.on {
            true => defaults.insert(params.project, true),
            false => defaults.remove(&params.project),
        };
        core.preferences.set("agentWorktreeDefaults", serde_json::to_value(defaults)?)?;
        core.view_changed();
        core.changes.changed(Topic::Preferences, None, None);
    },
    "theme": Watch, any_program => {
        let params: ThemeParams = serde_json::from_value(params).unwrap_or_default();
        match params.id {
            Some(theme_id) => answer(super::themes::palette(&crate::protocol::Preferences { theme_id, ..core.preferences.get() })),
            None => answer(core.look.palette()),
        }
    },
    "typeWhenReady": Full => {
        let params: TypeWhenReadyParams = parse(params)?;
        let session = core.session(params.id).ok_or("that terminal is not on this host")?;
        let typist = super::connection::typist(client);
        if !session.may_type(typist) {
            return Err(super::session::SETUP_LOCKED.into());
        }
        if let Some(draft) = super::session::typable(&params.draft) {
            super::session::type_when_ready(&session, draft, typist);
        }
        answer(json!({}))
    },
    "promptDraft": Full => {
        let params: SessionParams = parse(params)?;
        let session = core.session(params.id).ok_or("that terminal is not on this host")?;
        answer(json!({ "draft": super::session::prompt_draft(&session) }))
    },
    "reconnectSsh": Full => {
        let params: SessionParams = parse(params)?;
        let session = core.session(params.id).ok_or("that terminal is not on this host")?;
        blocking(move || {
            super::session::reconnect_ssh(&session);
            Ok(())
        })
        .await?;
        answer(json!({}))
    },
    "newTerminal": Full => answer(super::spare::new_terminal(&core, parse(params)?).await?),
    "showTerminals": Full => answer(super::spare::show_terminals(&core).await?),
    "stripShell": Full => answer(super::spare::strip_shell(&core, parse(params)?).await?),
    "openSshTerminal": Full => answer(super::spare::open_ssh_terminal(&core, parse(params)?).await?),
    "takeSpareShell": Local => answer(super::spare::take(&core, parse(params)?)),
    "themes": Watch => answer(super::themes::summaries(&core.preferences.get())),
    "themePalette": Watch => {
        let params: ThemePaletteParams = parse(params)?;
        answer(super::themes::palette_of(params.theme))
    },
    "themeFromColours": Watch => {
        let params: ThemeFromColoursParams = parse(params)?;
        let theme = super::themes::from_colours(&params.name, &params.colours, params.accent.as_deref());
        answer(theme.ok_or("a theme needs 21 colours, each #rrggbb, the selection's may be empty")?)
    },
    "themeFromPixels": Watch => {
        let params: super::wallpaper::PixelsParams = parse(params)?;
        let rgba = super::wallpaper::pixels(&params)?;
        let (colours, accent) = super::wallpaper::colours(&rgba)?;
        let theme = super::themes::from_colours(&params.name, &colours, Some(&accent));
        answer(theme.ok_or("the wallpaper gave no theme")?)
    },
    "codeHostPulls": Watch => answer(super::codehost::pulls(&core, client.id, parse(params)?).await),
    "codeHostRuns": Watch => answer(super::codehost::runs(&core, client.id, parse(params)?).await),
    "codeHostIssues": Watch => answer(super::codehost::issues(&core, client.id, parse(params)?).await),
    "codeHostReleases": Watch => answer(super::codehost::releases(&core, client.id, parse(params)?).await),
    "codeHostPull": Watch => answer(super::codehost::pull(&core, client.id, parse(params)?).await),
    "codeHostPullFiles": Watch => answer(super::codehost::pull_files(&core, client.id, parse(params)?).await),
    "codeHostLink": Watch => answer(super::codehost::link(&core, parse(params)?).await),
    "codeHostCheckout": Person => answer(super::codehost::checkout(&core, parse(params)?).await),
    "codeHostNewPull": Watch => answer(super::codehost::new_pull(&core, parse(params)?).await),
    "codeHostTrackedContext" CodeHostTrackedContext (super::codehost::TrackedParams) -> plugins::Outcome: Watch => {
        super::codehost::tracked_context(&core, params).await
    },
    "codeHostWorkOn": Person => answer(super::codehost::work_on(&core, parse(params)?).await),
    "codeHostTracked" CodeHostTracked (super::codehost::ReleasesParams) -> plugins::Outcome: Watch => {
        super::codehost::tracked(&core, client.id, params).await
    },
    "codeHostRun": Watch => answer(super::codehost::run(&core, client.id, parse(params)?).await),
    "codeHostJobLog": Watch => answer(super::codehost::job_log(&core, client.id, parse(params)?).await),
    "codeHostAway": Watch => {
        super::codehost::away(&core, client.id);
        answer(Value::Null)
    },
    "hosts": Watch => answer(super::hosts::rows(&core)),
    "hostInstalledFrom": Person => answer(super::hosts::installed_from(&core, parse(params)?)?),
    "installHost": Person => answer(super::hosts::install::install_host(&core, parse(params)?).await?),
    "hostSetup": Person => answer(super::hosts::install::host_setup(&core, parse(params)?).await?),
    "linkHost": Person => answer(super::hosts::link_host(&core, parse(params)?).await?),
    "openOnHost": Person => answer(super::hosts::open::open_on(&core, parse(params)?).await?),
    "sendSession": Person => answer(super::hosts::transfer::send_session(&core, parse(params)?).await?),
    "requireApproval" RequireApproval (NoParams) -> remote::ApprovalSetting: Person => remote::ApprovalSetting {
        require_approval: core.remote.requires_approval(),
    },
    "setRequireApproval" SetRequireApproval (remote::ApprovalSetting) -> RemoteStatus: Person => {
        remote::set_require_approval(&core, params)?
    },
    "questions" Questions (NoParams) -> Vec<AgentQuestion>: Watch, any_program => questions::views(&core),
    "answerQuestion" AnswerQuestion (questions::AnswerParams) -> (): Full => questions::answer(&core, params).await?,
    "answerQuestions" AnswerQuestions (questions::AnswersParams) -> (): Full => questions::answer_all(&core, params).await?,
    "agentMessages": Watch, any_program => answer(agent_messages(&core, parse(params)?).await?),
    "agentMessageRows" AgentMessageRows (AgentMessagesParams) -> AgentMessageRows: Watch, any_program => {
        agent_message_rows(&core, params).await?
    },
    "boards" Boards (NoParams) -> Vec<board::BoardView>: Watch, any_program => board::boards(&core),
    "board" Board (board::BoardParams) -> board::BoardView: Watch, any_program => board::board(&core, params).await?,
    "boardAdd": Full => answer(board::person_add(&core, params).await?),
    "boardAddTicket": Full => answer(board::person_add_ticket(&core, params).await?),
    "boardUpdate": Full => answer(board::person_update(&core, params).await?),
    "boardRemove": Full => answer(board::person_remove(&core, params).await?),
    "boardSettings": Full => answer(board::person_settings(&core, params).await?),
    "boardRetry": Full => answer(board::person_retry(&core, params).await?),
    "edits": Watch, any_program => answer(edits(&core, parse(params)?).await?),
    "changeOwners": Watch, any_program => answer(change_owners(&core, parse(params)?).await?),
    "commitStaged": Person => answer(commit_staged(&core, parse(params)?).await?),
    "commitOwners": Watch, any_program => answer(who_wrote::call(&core, parse(params)?).await?),
    "commitLines": Watch, any_program => answer(who_wrote::lines(&core, parse(params)?).await?),
    "inbox": Watch, any_program => answer(inbox(&core).await?),
    "digest": Watch, any_program => answer(digest(&core, parse(params)?).await?),
    "ports": Person => {
        let params: PortsParams = parse(params)?;
        answer(super::ports::project_ports(&core, client.id, params.project).await?)
    },
    "testResults": Person => {
        let params: ProjectParams = parse(params)?;
        answer(super::tasks::module_tests(&core, params.project).await?)
    },
    "projectConfig": Person => {
        let params: ProjectParams = parse(params)?;
        answer(super::tasks::project_config(&core, params.project).await?)
    },
    "startPreview": Person => {
        let params: ProjectParams = parse(params)?;
        answer(super::tasks::start_preview(&core, params.project).await?)
    },
    "trustProject": Person => {
        let params: TrustProjectParams = parse(params)?;
        let answered_by = answered_by(&core, &client.peer);
        answer(super::tasks::trust_project(&core, params.project, params.fingerprint, answered_by).await?)
    },
    "tasks": Person => {
        let params: ProjectParams = parse(params)?;
        answer(super::tasks::list(&core, params.project).await?)
    },
    "taskRuns": Person => answer(super::tasks::runs(&core)),
    "taskStart": Person => answer(super::tasks::start(&core, parse(params)?).await?),
    "taskRerunFailed": Person => answer(super::tasks::rerun_failed(&core, parse(params)?).await?),
    "taskStop": Person => {
        super::harness::stop_runs(&core, parse(params)?).await;
        answer(())
    },
    "trustAsks": Person => answer(super::tasks::asks(&core)),
    "showCi": Watch, any_program => {
        super::codehost::ci::show(&core, client.id, parse(params)?);
        answer(())
    },
    "answerTrust": Person => {
        let params: TrustAnswerParams = parse(params)?;
        let answered_by = answered_by(&core, &client.peer);
        answer(super::tasks::answer_trust(&core, &params.id, params.trust, answered_by)?)
    },
    "addTask": Person => {
        let answered_by = answered_by(&core, &client.peer);
        answer(super::tasks::add(&core, parse(params)?, answered_by).await?)
    },
    "removeTask": Person => answer(super::tasks::remove(&core, parse(params)?).await?),
    "setCheck": Person => answer(super::tasks::set_check(&core, parse(params)?).await?),
    "agentModels": Local => {
        let params: AgentModelsParams = parse(params)?;
        answer(super::chat::offered::models(&params.agent))
    },
    "plugin": Person => answer(plugins::window_call(&core, parse(params)?).await),
    "pluginManifests": Person => answer(plugins::manifests(&core)),
    "releaseNotes": Watch => answer(super::about::release_notes(parse(params)?).await?),
    "releaseAvatars": Watch => answer(super::about::release_avatars(parse(params)?).await),
    "hostTools": Watch => answer(super::about::tools().await?),
    "whatsNew": Watch => answer(super::about::whats_new(&core)),
    "cliInstall": Watch => answer(super::cli_install::status().await?),
    "installCli": Person => answer(super::cli_install::install().await?),
    "renameAccountDevice": Person => answer(super::hosts::account::rename_device(&core, parse(params)?).await?),
    "removeAccountDevice": Person => answer(super::hosts::account::remove_device(&core, parse(params)?).await?),
    "accountPhones": Person => answer(super::hosts::account::phones(&core).await?),
    "showFiles": Watch => {
        super::open_files::show(&core, client.id, parse(params)?)?;
        answer(())
    },
    "pluginView": Watch => answer(super::plugin_views::view(&core, client.id, parse(params)?).await),
    "pluginViewAway": Watch => {
        super::plugin_views::away(&core, client.id, parse(params)?);
        answer(())
    },
    "pluginViewsRead": Watch => {
        super::plugin_views::read_again(&core, parse(params)?);
        answer(())
    },
    "plugins": Watch => answer(plugins::listed(&core).await),
    "pluginLink": Watch => answer(plugins::link(&core, parse(params)?).await),
    "pluginSetDisabled": Person => {
        plugins::set_disabled(&core, parse(params)?);
        answer(())
    },
    "pluginStream": Person => answer(plugins::stream(&core, connection_of(&core, client)?, parse(params)?)?),
    "pluginSignIn": Person, any_program => {
        not_an_agent(client).await?;
        answer(plugins::sign_in(&core, connection_of(&core, client)?, parse(params)?)?)
    },
    "signInCallback": Person, any_program => {
        not_an_agent(client).await?;
        answer(plugins::sign_in_callback(&core, parse(params)?)?)
    },
}

/// The file tree's and the editor's methods on this host's disk, each read
/// answering with at most a piece of a file so other replies keep moving.
async fn files(core: &Core, method: &str, params: Value) -> CoreResult<Value> {
    let method = method.to_owned();
    let ran = blocking(move || {
        crate::files::run(&method, params, crate::files::PIECE_BYTES)
            .map_err(|error| CoreError::from(error.to_string()))
    })
    .await?;
    if let Some(path) = ran.saved {
        core.edits.saved(path);
    }
    Ok(ran.answer)
}

/// Refuses a program an agent started: it could sign the person's plugin in
/// to an account of its own choosing.
async fn not_an_agent(client: &ClientConn) -> CoreResult<()> {
    let Some(pid) = client.pid else {
        return Ok(());
    };
    match blocking(move || Ok(super::callers::agent_of(pid))).await? {
        Some(_) => {
            Err("an agent cannot sign the person in to a service; the person runs this".into())
        }
        None => Ok(()),
    }
}

#[derive(Deserialize)]
struct StreamParams {
    /// None stops every stream on the connection, as a reloaded page does.
    stream: Option<u64>,
}

#[derive(Deserialize)]
struct SearchStreamParams {
    stream: u64,
    #[serde(flatten)]
    search: super::search::SearchParams,
}

/// The connection a call came on, for a stream to send its parts down.
fn connection_of(core: &Core, client: &ClientConn) -> CoreResult<Arc<ClientConn>> {
    core.client_conn(client.id)
        .ok_or_else(|| CoreError::from("this connection is closing"))
}

/// Starts a search whose files arrive as the client's stream `stream`.
fn search_stream(
    core: &Core,
    client: &ClientConn,
    params: SearchStreamParams,
) -> CoreResult<Value> {
    let stream = core
        .streams
        .open(connection_of(core, client)?, params.stream)?;
    let owner = client.id.to_string();
    tokio::task::spawn_blocking(move || {
        let found = super::search::search(params.search, &owner, &|file| {
            stream.push(json!({ "file": file }))
        });
        match found {
            Ok(summary) => stream.finish(Some(json!({ "summary": summary }))),
            Err(message) => stream.fail(json!({ "category": "search", "message": message })),
        }
    });
    Ok(json!({ "stream": params.stream }))
}

pub(super) fn parse<T: for<'de> Deserialize<'de>>(params: Value) -> CoreResult<T> {
    serde_json::from_value(params)
        .map_err(|error| CoreError::from(format!("the call's params do not fit: {error}")))
}

#[derive(Deserialize)]
struct CommandUsed {
    id: String,
}

/// The name of the device the person acted from, when it was not this host's own window.
fn answered_by(core: &Core, peer: &Peer) -> Option<String> {
    let device = peer.device_id()?;
    Some(core.remote.device_name(&device).unwrap_or(device))
}

fn answer(result: impl Serialize) -> CoreResult<Value> {
    Ok(serde_json::to_value(result)?)
}

fn signed((key, signature): (String, String)) -> Value {
    json!({ "key": key, "signature": signature })
}

#[derive(Debug, Deserialize)]
struct NonceParams {
    nonce: String,
}

#[derive(Serialize, Deserialize)]
struct NameThemeParams {
    theme: NameTheme,
}

#[derive(Deserialize)]
struct ProjectParams {
    project: String,
}

#[derive(Deserialize)]
struct PortsParams {
    project: Option<String>,
}

#[derive(Deserialize)]
struct TrustProjectParams {
    project: String,
    fingerprint: String,
}

#[derive(Deserialize)]
struct SessionParams {
    id: crate::protocol::SessionId,
}

/// A draft to paste into a terminal once it is ready, left at the prompt for
/// the person to send.
#[derive(Deserialize)]
struct TypeWhenReadyParams {
    id: crate::protocol::SessionId,
    draft: String,
}

/// Which theme to answer for; the person's pick when none is named.
#[derive(Deserialize, Default)]
struct ThemeParams {
    id: Option<String>,
}

#[derive(Deserialize)]
struct ThemePaletteParams {
    theme: sikemux_wire::theme::Theme,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThemeFromColoursParams {
    name: String,
    colours: Vec<String>,
    accent: Option<String>,
}

/// The drafts to read: those under `keys`, else every one.
#[derive(Deserialize, Default)]
struct DraftsParams {
    keys: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct DiagramParams {
    key: String,
    /// The arrangement to keep; without one the kept arrangement is forgotten.
    #[serde(default)]
    layout: Option<Value>,
}

#[derive(Serialize, Deserialize)]
pub struct SetDraftParams {
    pub key: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra: Option<Value>,
    /// The draft's revision the app last read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<u64>,
}

#[derive(Deserialize)]
struct SetPreferenceParams {
    key: String,
    value: Value,
}

#[derive(Deserialize)]
struct AgentModelsParams {
    agent: String,
}

#[derive(Deserialize)]
struct TrustAnswerParams {
    id: String,
    trust: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMessagesParams {
    project: String,
    #[serde(default)]
    limit: Option<usize>,
}

/// A project's messages between agents, one row each, newest first, under the
/// project's own path.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMessageRows {
    project: String,
    rows: Vec<messages::log::Row>,
}

/// The project's log of messages between agents, newest first, under the
/// project's own path, which keys it in the inbox's `agentMessages`.
async fn agent_messages(core: &Arc<Core>, params: AgentMessagesParams) -> CoreResult<Value> {
    let path = Path::new(&params.project);
    if !path.is_absolute() {
        return Err("project must be the project's absolute path".into());
    }
    let path = path.to_owned();
    let core = core.clone();
    blocking(move || {
        let project = core.workspaces.project_of(&path);
        let entries = messages::logged(&core, &project, params.limit);
        Ok(json!({ "project": project, "entries": entries }))
    })
    .await
}

/// The project's messages between agents folded into one row each, newest first.
async fn agent_message_rows(
    core: &Arc<Core>,
    params: AgentMessagesParams,
) -> CoreResult<AgentMessageRows> {
    let path = Path::new(&params.project);
    if !path.is_absolute() {
        return Err("project must be the project's absolute path".into());
    }
    let path = path.to_owned();
    let core = core.clone();
    blocking(move || {
        let project = core.workspaces.project_of(&path);
        let rows = super::messages::logged_rows(&core, &project);
        Ok(AgentMessageRows { project, rows })
    })
    .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShareChatParams {
    agent_id: String,
    lifetime_ms: u64,
    title: Option<String>,
}

#[derive(Deserialize)]
struct StopShareParams {
    id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChatFileParams {
    path: PathBuf,
    /// For a picture shown small: the longest side, in pixels, to send it scaled to.
    #[serde(default)]
    max_edge: Option<u32>,
}

#[derive(Serialize)]
struct ChatFile {
    /// Set when the file was sent as a scaled copy, which is a JPEG.
    #[serde(skip_serializing_if = "Option::is_none")]
    mime: Option<&'static str>,
    data: String,
}

/// A file a chat's message carried, in base64. Any device may read the
/// pictures pasted into chats; one that can drive sessions can already read
/// anything through a terminal, so it may read files dropped from elsewhere.
async fn chat_file(core: &Core, peer: &Peer, params: ChatFileParams) -> CoreResult<ChatFile> {
    let pasted = core
        .listening
        .get()
        .and_then(|listening| listening.config.attachment_dir.clone());
    let path = params.path;
    let kept = match pasted {
        Some(dir) => {
            let path = path.clone();
            blocking(move || Ok(attachments::is_kept(&dir, &path))).await?
        }
        None => false,
    };
    if !kept {
        core.permit(peer, Needs::Full).map_err(|_| {
            CoreError::from("this device can open only the pictures pasted into chats")
        })?;
    }
    let max_edge = params.max_edge;
    blocking(move || {
        if let Some(data) = max_edge.and_then(|edge| attachments::thumbnail(&path, edge)) {
            return Ok(ChatFile {
                mime: Some("image/jpeg"),
                data,
            });
        }
        Ok(ChatFile {
            mime: None,
            data: attachments::read(&path)?,
        })
    })
    .await
}

#[derive(Deserialize)]
struct KeepPastedParams {
    name: String,
    mime: String,
    data: String,
}

#[derive(Serialize)]
struct Kept {
    path: PathBuf,
}

/// A file pasted into a terminal, kept where pictures pasted into chats are, so the
/// program in the terminal can be given its path.
async fn keep_pasted(core: &Core, params: KeepPastedParams) -> CoreResult<Kept> {
    let dir = core
        .listening
        .get()
        .and_then(|listening| listening.config.attachment_dir.clone())
        .ok_or_else(|| CoreError::from("this host has nowhere to keep files"))?;
    let path =
        blocking(move || attachments::save(&dir, &params.name, &params.mime, &params.data)).await?;
    Ok(Kept { path })
}

#[derive(Deserialize)]
struct PageParams {
    id: String,
}

#[derive(Serialize)]
struct PageHtml {
    html: String,
}

/// A page an agent showed with `page_show`, as a chat shows it in the reply.
async fn page(core: &Core, params: PageParams) -> CoreResult<PageHtml> {
    if !crate::pages::is_page_id(&params.id) {
        return Err("that is not a page's id".into());
    }
    let dir = core
        .harness
        .data_dir
        .clone()
        .ok_or("this core keeps no pages")?
        .join(crate::pages::FOLDER);
    let html = blocking(move || {
        std::fs::read_to_string(dir.join(format!("{}.html", params.id)))
            .map_err(|_| CoreError::from("that page is gone"))
    })
    .await?;
    Ok(PageHtml { html })
}

/// A page of a chat's rows: those before or after an event, or around the row holding one.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChatPageParams {
    pub agent_id: String,
    pub feed: String,
    pub before: Option<u64>,
    pub after: Option<u64>,
    pub around: Option<u64>,
    /// A subagent's rows rather than the chat's own.
    pub session: Option<String>,
    pub turns: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDetailParams {
    agent_id: String,
    tool_call_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NameAgentParams {
    agent_id: String,
    cwd: PathBuf,
    provider: Option<String>,
    resume_id: Option<String>,
    /// How the app draws it, kept for when it is opened again.
    buddy: Option<AgentBuddy>,
    title: Option<String>,
}

/// An agent's name, with the character it wore before it closed when it
/// opens again under a name the person chose.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NamedAgent {
    #[serde(flatten)]
    named: AgentName,
    #[serde(skip_serializing_if = "Option::is_none")]
    buddy: Option<AgentBuddy>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActivitySummaryParams {
    /// Minutes the person's clock is ahead of UTC, which places each event on their own day.
    utc_offset_minutes: i64,
    project: Option<String>,
}

#[derive(Deserialize)]
struct ActivityPriceParams {
    model: String,
    price: Option<super::activity_log::PriceOverride>,
}

async fn activity_summary(
    core: &Arc<Core>,
    params: ActivitySummaryParams,
) -> CoreResult<super::activity_log::ActivitySummary> {
    let core = core.clone();
    blocking(move || {
        Ok(core.activity_log.summary(
            params.utc_offset_minutes * 60_000,
            params.project.as_deref(),
        ))
    })
    .await
}

#[derive(Deserialize)]
struct AccountPictureParams {
    hash: String,
}

/// A person's message `message_id` written again as `text`, sent with the files at `paths`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditParams {
    agent_id: String,
    message_id: String,
    text: String,
    #[serde(default)]
    paths: Vec<String>,
}

/// Takes the chat back to before the message and sends the new text in its place, as the
/// window's own edit does, starting the chat first when it was stopped.
async fn edit_message(core: &Arc<Core>, params: EditParams) -> CoreResult<()> {
    super::chat::wake_for_message(core, &params.agent_id, &mut []).await?;
    let EditParams {
        agent_id,
        message_id,
        text,
        paths,
    } = params;
    super::chat::edit(core, &agent_id, message_id, text, paths, Vec::new()).await
}

/// Messages waiting behind an agent's turn, by id.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueuedParams {
    pub agent_id: String,
    pub ids: Vec<String>,
    /// A message sent with them, after them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<super::chat::queue::Message>,
}

/// What a call that takes nothing is sent; whatever a client sends reads as nothing.
#[derive(Serialize)]
pub struct NoParams {}

impl<'de> Deserialize<'de> for NoParams {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        serde::de::IgnoredAny::deserialize(deserializer)?;
        Ok(NoParams {})
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentParams {
    agent_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TriggerParams {
    trigger: u64,
}

/// A saved agent to start; one moved to another agent before it ran waits for the person.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartParams {
    agent_id: String,
    #[serde(default)]
    waits_for_person: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TerminalParams {
    agent_id: String,
    cols: u16,
    rows: u16,
}

/// The name the core gave an agent the app starts or opens again, given now
/// when it has none.
async fn name_agent(core: &Arc<Core>, params: NameAgentParams) -> CoreResult<NamedAgent> {
    if !crate::cli::protocol::is_tool_agent_id(&params.agent_id) {
        return Err("that is not an agent id".into());
    }
    if !params.cwd.is_absolute() {
        return Err("an agent's folder must be an absolute path".into());
    }
    let core = core.clone();
    blocking(move || {
        let (named, kept) = core.name_agent(
            &params.agent_id,
            &params.cwd,
            params.provider.as_deref(),
            params.resume_id.as_deref(),
        );
        let buddy = match kept {
            Some(kept) => kept,
            None => {
                core.names
                    .note_look(&params.agent_id, params.buddy, params.title);
                None
            }
        };
        Ok(NamedAgent { named, buddy })
    })
    .await
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClosedAgentsParams {
    pub cwd: PathBuf,
}

/// The closed agents of the project at `cwd` that keep the names the person
/// gave them, newest first.
async fn closed_agents(
    core: &Arc<Core>,
    params: ClosedAgentsParams,
) -> CoreResult<Vec<ClosedAgent>> {
    let core = core.clone();
    blocking(move || {
        let project = core.workspaces.project_of(&params.cwd);
        Ok(core.names.closed(&project, core.open_agents()))
    })
    .await
}

/// Whether a new chat in the project at the folder `project` starts in a worktree of its own.
#[derive(Serialize, Deserialize)]
pub struct WorktreeDefaultParams {
    pub project: String,
    pub on: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptChatParams {
    agent_id: String,
    #[serde(default)]
    message_id: Option<String>,
    text: String,
    #[serde(default)]
    paths: Vec<String>,
    #[serde(default)]
    context: Vec<crate::protocol::ChatContext>,
}

/// Sends the person's message as `AcpPrompt` does, for a client whose own request
/// carries no context: the agents, earlier chats and issues the message names.
async fn prompt_chat(core: &Arc<Core>, params: PromptChatParams) -> CoreResult<()> {
    let PromptChatParams {
        agent_id,
        message_id,
        text,
        paths,
        mut context,
    } = params;
    if core.workspaces.in_terminal(&agent_id) {
        messages::from_person(core, &agent_id, text)?;
    } else {
        super::chat::wake_for_prompt(core, &agent_id, &mut context).await?;
        super::chat::prompt(core, &agent_id, message_id, text, paths, context)?;
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RenameAgentParams {
    agent_id: String,
    name: String,
}

async fn rename_agent(core: &Arc<Core>, params: RenameAgentParams) -> CoreResult<AgentName> {
    let core = core.clone();
    blocking(move || {
        let named = core
            .names
            .rename(&params.agent_id, &params.name, core.open_agents())?;
        core.view_changed();
        Ok(named)
    })
    .await
}

/// The names of the agents running here and of the workers they started on
/// linked hosts, each with the agent that started it, so a device can list
/// workers after their lead.
async fn agent_names(core: &Arc<Core>) -> CoreResult<Vec<AgentName>> {
    let core = core.clone();
    blocking(move || {
        let mut agents = core.running_agents();
        agents.extend(core.hosts.team.ids());
        Ok(core.names.of(&agents))
    })
    .await
}

#[derive(Deserialize)]
struct CheckoutParams {
    dir: PathBuf,
    #[serde(default)]
    staged: bool,
}

/// Which agent wrote each uncommitted line of a checkout.
async fn change_owners(core: &Arc<Core>, params: CheckoutParams) -> CoreResult<Owners> {
    let core = core.clone();
    blocking(move || Ok(edits::owners_at(&core, &params.dir, params.staged)?)).await
}

#[derive(Deserialize)]
struct CommitParams {
    dir: PathBuf,
    message: String,
}

/// Commits what the person staged in the git panel, with the footer naming
/// the agents whose lines it holds. What git said.
async fn commit_staged(core: &Arc<Core>, params: CommitParams) -> CoreResult<String> {
    let core = core.clone();
    blocking(move || {
        let output = edits::footer::commit(&core, &params.dir, &[], &params.message, None)?;
        let said = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();
        if output.status.success() {
            core.activity_log
                .record_commit(&params.dir.to_string_lossy());
            Ok(said(&output.stdout))
        } else if output.stderr.trim_ascii().is_empty() {
            Err(said(&output.stdout).into())
        } else {
            Err(said(&output.stderr).into())
        }
    })
    .await
}

/// The files agents changed in a project and the lines they changed, by
/// agent, chat run, turn or path, oldest first.
async fn edits(core: &Arc<Core>, filter: Filter) -> CoreResult<Vec<Edit>> {
    let core = core.clone();
    blocking(move || {
        let project = core.workspaces.project_of(&filter.project);
        Ok(core.edits.read(&project, &filter))
    })
    .await
}

/// What waits on the person, ranked as devices' views list it.
async fn inbox(core: &Arc<Core>) -> CoreResult<sikemux_wire::inbox::Inbox> {
    let core = core.clone();
    blocking(move || {
        let (chats, sessions) = core.ranked_infos();
        Ok(inbox::view(&core, &chats, &sessions))
    })
    .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DigestParams {
    since: Option<u64>,
}

/// What finished, failed and waits since the person last left, or `since`.
async fn digest(
    core: &Arc<Core>,
    params: DigestParams,
) -> CoreResult<sikemux_wire::inbox::InboxDigest> {
    let core = core.clone();
    blocking(move || Ok(inbox::digest(&core, params.since))).await
}
