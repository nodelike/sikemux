//! The core's methods a client reaches by name through `Request::Call`. A
//! device gains a method here without its native code changing, so one a
//! released app calls is never removed or renamed.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::access::{Needs, Program};
use super::activity_log::Counted;
use super::chat::calls as chat_calls;
use super::connection::{blocking, ClientConn};
use super::workspace;
use super::{
    agent_accounts, agent_sign_in, attachments, board, edits, goal_summary, goals, inbox, memory,
    messages, model_providers, names, plugins, questions, remote, saved, saved_sessions, share,
    ship, skill_library, who_wrote, Core, CoreError, CoreResult,
};
use sikemux_view::catalog::CatalogAgent;
use sikemux_view::chat::ToolDetail;
use sikemux_view::memory::MemoryView;
use sikemux_view::prefs::Preferences;
use sikemux_view::questions::AgentQuestion;
use sikemux_wire::protocol::RemoteStatus;

/// What one row of the table says about its method: what a paired device needs, which
/// programs on this host may call it, and whether its use goes in the usage report.
#[derive(Clone, Copy)]
pub(crate) struct Row {
    pub needs: Needs,
    pub program: Program,
    pub counted: Counted,
}

impl Row {
    pub(super) const fn of(needs: Needs) -> Self {
        Self {
            needs,
            program: Program::App,
            counted: Counted::No,
        }
    }

    /// Any program on this host may call it, rather than only Sikemux's own window or notch.
    pub(super) const fn any_program(self) -> Self {
        Self {
            program: Program::Anyone,
            ..self
        }
    }

    pub(super) const fn counted(self) -> Self {
        Self {
            counted: Counted::Yes,
            ..self
        }
    }

    /// Counted under the name its params give in `field`.
    const fn counted_by(self, field: &'static str) -> Self {
        Self {
            counted: Counted::ByField(field),
            ..self
        }
    }
}

/// Each method once: its [`Row`] and what it runs. A method whose params and answer are
/// named, as `"m" Marker (Params) -> Answer`, runs on its params read as that type and
/// answers that type, which clients are generated to call it with, and is `call::Marker` to
/// a client in Rust. The `git.*` methods have the same rows in `git.rs`.
macro_rules! calls {
    ($core:ident, $client:ident, $method:ident, $params:ident;
     $($name:literal $($marker:ident ($takes:ty) -> $gives:ty)?: $needs:ident $(, $flag:ident $(($arg:expr))?)* => $run:expr,)*) => {
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

        /// A method no row names goes no further than the app, which is told it is unknown.
        pub(crate) fn row(method: &str) -> Row {
            match method {
                $($name => Row::of(Needs::$needs) $(.$flag($($arg)?))*,)*
                method => super::git::row(method).unwrap_or(Row::of(Needs::NoDevice)),
            }
        }

        pub(crate) async fn call(
            $core: Arc<Core>,
            $client: &ClientConn,
            $method: &str,
            $params: Value,
        ) -> CoreResult<Value> {
            match $method {
                $($name => calls!(@run $params, $run $(, $takes, $gives)?),)*
                method => super::git::call(&$core, &$client.peer, method, $params).await,
            }
        }
    };
    (@run $params:ident, $run:expr) => { $run };
    (@run $params:ident, $run:expr, $takes:ty, $gives:ty) => {{
        #[allow(unused_variables)]
        let $params: $takes = parse($params)?;
        let gives: $gives = $run;
        answer(gives)
    }};
}

calls! { core, client, method, params;
    "chatFile": Watch, any_program => answer(attachments::chat_file(&core, &client.peer, parse(params)?).await?),
    "keepPasted": Full => answer(attachments::keep_pasted(&core, parse(params)?).await?),
    "page" ReadPage (super::harness::pages::PageParams) -> super::harness::pages::PageHtml: Watch, any_program => super::harness::read_page(&core, params).await?,
    "nameAgent": NoDevice => answer(names::name_agent(&core, parse(params)?).await?),
    "renameAgent" RenameAgent (names::RenameAgentParams) -> names::AgentName: Full, counted => names::rename_agent(&core, params).await?,
    "promptChat" PromptChat (chat_calls::PromptChatParams) -> (): Full => chat_calls::prompt_chat(&core, params).await?,
    "closedAgents" ClosedAgents (names::ClosedAgentsParams) -> Vec<names::ClosedAgent>: Full => names::closed_agents(&core, params).await?,
    "agentNames": Watch, any_program => answer(names::agent_names(&core).await?),
    "agentCatalog" AgentCatalog (NoParams) -> Vec<CatalogAgent>: Watch, any_program => super::catalog::view(),
    "agentNameTheme": Full => answer(names::name_theme(&core)),
    "setAgentNameTheme": Owner, counted => answer(names::set_name_theme(&core, parse(params)?)),
    "hostLoad": Watch, any_program => answer(super::load::now()),
    "stopChatIfIdle": NoDevice => answer(json!({ "stopped": super::lifecycle::sleep(&core, &parse::<AgentParams>(params)?.agent_id) })),
    "switchAgentView" SwitchAgentView (super::agent_view::AgentViewParams) -> Value: Owner => super::agent_view::switch(&core, params).await?,
    "sleepAgent": Owner, counted => answer(super::lifecycle::sleep_agent(&core, &parse::<AgentParams>(params)?.agent_id)?),
    "sleepIdleChats": NoDevice => answer(json!({ "slept": super::lifecycle::sleep_idle(&core) })),
    "startTerminalAgent": NoDevice, counted => super::launching::start_terminal_at(&core, parse(params)?).await,
    "commandUsed": NoDevice => {
        core.activity_log.command_used(&parse::<super::activity_log::CommandUsed>(params)?.id);
        answer(())
    },
    "composing": Owner => {
        super::chat::composing(&core, &parse::<AgentParams>(params)?.agent_id);
        answer(())
    },
    "startChat": NoDevice, counted => {
        let params: super::launching::StartParams = parse(params)?;
        answer(super::launching::start_saved(&core, &params.agent_id, params.waits_for_person).await?)
    },
    "resumeSession" ResumeSession (super::workspace::SessionChoice) -> super::launching::SessionTaken: Full, counted => super::launching::resume_session(&core, params).await?,
    "activitySummary": Watch, any_program => answer(super::activity_log::summary_for(&core, parse(params)?).await?),
    "setActivityPrice": Owner => answer(super::activity_log::set_price_for(&core, parse(params)?)?),
    "steerQueued" SteerQueued (chat_calls::QueuedParams) -> (): Full, counted => {
        super::chat::queue::steer(&core, &params.agent_id, params.ids, params.message).await?
    },
    "editMessage" EditMessage (chat_calls::EditParams) -> (): Full, counted => chat_calls::edit_message(&core, params).await?,
    "chatPage": Watch, any_program => {
        let params: chat_calls::ChatPageParams = parse(params)?;
        let reading = core.clone();
        answer(blocking(move || super::chat::page(&reading, params)).await?)
    },
    "chatFind" ChatFind (super::chat::find::FindParams) -> super::chat::find::FindAnswer: Watch, any_program, counted => {
        let (reading, client) = (core.clone(), client.id);
        blocking(move || super::chat::find(&reading, client, params)).await?
    },
    "chatToolDetail" ChatToolDetail (chat_calls::ToolDetailParams) -> ToolDetail: Watch, any_program => {
        super::chat::tool_detail(&core, &params.agent_id, &params.tool_call_id)?
    },
    "dropQueued" DropQueued (chat_calls::QueuedParams) -> (): Full => {
        super::chat::queue::remove(&core, &params.agent_id, &params.ids)?
    },
    "agentSessions" AgentSessions (saved::SessionSearchParams) -> Vec<crate::sessions::search::SessionMatch>: Full, any_program => {
        saved::call(&core, params).await?
    },
    "workspace": Watch, any_program => answer(core.workspace_view()),
    "agentManifests": NoDevice, any_program => answer(core.manifest_report()),
    "reloadAgentManifests": NoDevice => {
        answer(blocking(move || core.reload_manifests()).await?)
    },
    "configureAgentManifests": NoDevice => {
        let ManifestFolder { manifest_dir } = parse(params)?;
        answer(blocking(move || core.configure_manifests(manifest_dir)).await?)
    },
    "explainAgentDetection": NoDevice, any_program => {
        answer(super::agent::explain_of(core, parse::<AgentParams>(params)?.agent_id).await?)
    },
    "runCommand": NoDevice => answer(super::custom_command::run(parse(params)?).await?),
    "updateStatus": Watch, any_program => answer(core.updates.status()),
    "updateCheck": Owner => answer(super::updates::check(&core).await?),
    "updateInstall" UpdateInstall (super::updates::UpdateInstallParams) -> super::updates::UpdateStatus: Owner, counted => super::updates::install(&core, params)?,
    "updateCancel": Owner => answer(super::updates::cancel(&core)),
    "updateHost": NoDevice, any_program => answer(super::updates::update_host(&core).await?),
    "setHostUpdates" SetHostUpdates (super::updates::HostUpdatesParams) -> super::updates::UpdateStatus: Owner => super::updates::set_automatic(&core, params).await?,
    "runCustomCommand" RunCustomCommand (super::custom_command::RunCustomParams) -> super::custom_command::CommandRan: Owner, counted => super::custom_command::run_custom(&core, params).await?,
    "addProject" AddProject (workspace::AddProjectParams) -> sikemux_view::workspace::ProjectInfo: Owner => workspace::add_project(&core, params).await?,
    "removeProject" RemoveProject (workspace::RemoveProjectParams) -> (): Owner => workspace::remove_project(&core, params)?,
    "updateProject" UpdateProject (workspace::UpdateProjectParams) -> sikemux_view::workspace::ProjectInfo: Owner => workspace::update_project(&core, params)?,
    "moveProject" MoveProject (workspace::MoveProjectParams) -> (): Owner => workspace::move_project(&core, params)?,
    "setSpaces" SetSpaces (workspace::SpacesParams) -> (): Owner, counted => workspace::set_spaces(&core, params)?,
    "setPinnedChats" SetPinnedChats (workspace::PinnedParams) -> (): Owner => workspace::set_pinned_chats(&core, params)?,
    "pinChat" PinChat (workspace::PinChatParams) -> (): Owner, counted => workspace::pin_chat(&core, params)?,
    "recentChats" RecentChats (workspace::RecentChatsParams) -> workspace::RecentChats: Owner => workspace::recent_chats(&core, params).await?,
    "setAgentDefaults" SetAgentDefaults (workspace::DefaultsParams) -> (): Owner => workspace::set_defaults(&core, params).await?,
    "createAgent": Owner, counted => answer(workspace::create_agent(&core, parse(params)?)?),
    "updateAgent" UpdateAgent (workspace::UpdateAgentParams) -> sikemux_view::workspace::WorkspaceAgent: Owner => workspace::update_agent(&core, params)?,
    "moveAgentToProject" MoveAgentToProject (super::move_agent::MoveParams) -> sikemux_view::workspace::WorkspaceAgent: Owner, counted => super::move_agent::move_agent(&core, params).await?,
    "moveAgentToFolder" MoveAgentToFolder (super::move_agent::MoveToFolder) -> sikemux_view::workspace::WorkspaceAgent: Owner => super::move_agent::move_to_folder(&core, params).await?,
    "closeAgents" CloseAgents (workspace::CloseAgentsParams) -> (): Owner => workspace::close_agents(&core, params)?,
    "closeChat" CloseChat (AgentParams) -> (): Full => workspace::close_agent(&core, params.agent_id)?,
    "createAgentWorktree" CreateAgentWorktree (super::worktree::agent::AgentWorktreeParams) -> super::worktree::agent::MadeAgentWorktree: Owner, counted => {
        super::worktree::agent::for_agent(&core, params).await?
    },
    "agentReview" AgentReview (super::worktree::removal::WorktreeParams) -> super::worktree::removal::WorktreeReview: Watch, counted => super::worktree::removal::review(&core, params).await?,
    "worktreeMerged" WorktreeMerged (super::worktree::removal::WorktreeParams) -> bool: Watch => super::worktree::removal::merged(&core, params).await?,
    "worktreeRemoval" WorktreeRemoval (super::worktree::removal::WorktreeParams) -> super::worktree::removal::WorktreeRemovalInfo: Owner => super::worktree::removal::removal(&core, params).await?,
    "removeAgentWorktree" RemoveAgentWorktree (super::worktree::removal::RemoveWorktreeParams) -> super::worktree::removal::WorktreeRemoved: Owner, counted => super::worktree::removal::remove(&core, params).await?,
    "agentWorktreeAt": Full => answer(super::worktree::agent::at(parse(params)?).await?),
    "savedSessions": Full, any_program => answer(saved_sessions::list(parse(params)?).await?),
    "recentSessions": Full, any_program => answer(saved_sessions::recent(parse(params)?).await?),
    "renameSession": Owner => answer(saved_sessions::rename(parse(params)?).await?),
    "deleteSession": Owner => answer(saved_sessions::delete(parse(params)?).await?),
    "renameRecent" RenameRecent (saved_sessions::RecentRenamed) -> (): Owner => saved_sessions::rename_recent(&core, params).await?,
    "deleteRecent" DeleteRecent (saved_sessions::RecentChat) -> (): Owner => saved_sessions::delete_recent(&core, params).await?,
    "relocateSession": Owner => answer(saved_sessions::relocate(parse(params)?).await?),
    "skillsOverview": Full, any_program => skill_library::overview(&core).await,
    "skillsAdded": Full, any_program => skill_library::added(&core).await,
    "createSkill": Owner, counted => skill_library::create(&core, parse(params)?).await,
    "installSkill": Owner, counted => skill_library::install(&core, parse(params)?).await,
    "removeSkill": Owner => skill_library::remove(&core, parse(params)?).await,
    "adoptSkill": Owner, counted => skill_library::adopt(&core, parse(params)?).await,
    "agentSignIns": Full, any_program => answer(agent_sign_in::pages()),
    "agentSignIn": Owner, counted => answer(agent_sign_in::sign_in(&core, parse(params)?).await?),
    "signInLauncher": Owner => answer(agent_sign_in::sign_in_launcher(&core, parse(params)?).await?),
    "agentSignInCode": Owner => answer(agent_sign_in::sign_in_code(parse(params)?).await?),
    "cancelAgentSignIn": Owner => {
        agent_sign_in::cancel(parse(params)?);
        answer(())
    },
    "agentSignOut": Owner => answer(agent_sign_in::sign_out(parse(params)?).await?),
    "modelProviders" ModelProviders (NoParams) -> Vec<model_providers::ModelProvider>: Full, any_program => model_providers::list(&core).await?,
    "modelProviderModels" ModelProviderModels (model_providers::ProviderParams) -> model_providers::ServedModels: Full, any_program => model_providers::models(&core, params).await?,
    "modelHelper" ModelHelper (NoParams) -> Option<crate::launch::providers::Helper>: Full, any_program => model_providers::helper(&core).await?,
    "modelProviderCheck" ModelProviderCheck (model_providers::CheckParams) -> model_providers::Check: Owner => model_providers::check(&core, params).await?,
    "modelProviderConnect" ModelProviderConnect (model_providers::ConnectParams) -> (): Owner, counted => model_providers::connect(&core, params).await?,
    "modelProviderDisconnect" ModelProviderDisconnect (model_providers::ProviderParams) -> (): Owner => model_providers::disconnect(&core, params).await?,
    "modelProviderRead" ModelProviderRead (model_providers::adding::Entered) -> model_providers::adding::Reading: Owner => model_providers::read(&core, params).await?,
    "modelProviderAdd" ModelProviderAdd (model_providers::adding::Entered) -> model_providers::ModelProvider: Owner => model_providers::add(&core, params).await?,
    "modelProviderRemove" ModelProviderRemove (model_providers::ProviderParams) -> (): Owner => model_providers::remove(&core, params).await?,
    "setModelHelper" SetModelHelper (model_providers::HelperParams) -> (): Owner => model_providers::set_helper(&core, params).await?,
    "agentAccounts": Full, any_program => answer(core.workspaces.accounts().unwrap_or_default().with_built_ins()),
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
    "openTab" OpenTab (sikemux_view::layout::OpenTab) -> String: Full => super::layout::open_tab(&core, params)?,
    "keepTab" KeepTab (super::layout::TabParams) -> (): Full => super::layout::keep_tab(&core, params)?,
    "closeTab" CloseTab (super::layout::TabParams) -> (): Full => super::layout::close_tab(&core, params)?,
    "activateTab" ActivateTab (super::layout::TabParams) -> (): Full => super::layout::activate_tab(&core, params)?,
    "moveTab" MoveTab (super::layout::TabParams) -> (): Full => super::layout::move_tab(&core, params)?,
    "closeTabs" CloseTabs (super::layout::TabsParams) -> (): Full => super::layout::close_tabs(&core, params)?,
    "renameTab" RenameTab (super::layout::TabParams) -> (): Full => super::layout::rename_tab(&core, params)?,
    "duplicateTab" DuplicateTab (super::layout::TabParams) -> String: Full, counted => super::layout::duplicate_tab(&core, params).await?,
    "activateProject" ActivateProject (super::layout::ActivateProjectParams) -> (): Full => super::layout::activate_project(&core, params)?,
    "openPaths" OpenPaths (super::layout::OpenPaths) -> Vec<super::layout::PathOpened>: Owner => super::layout::open_paths(&core, params).await?,
    "chooseHost" ChooseHost (super::layout::HostParams) -> (): Full => super::layout::choose_host(&core, params).await?,
    "splitPane" SplitPane (super::layout::SplitPane) -> String: Full, counted => super::layout::split_pane(&core, params)?,
    "closePane" ClosePane (super::layout::PaneParams) -> (): Full => super::layout::close_pane(&core, params)?,
    "separatePane" SeparatePane (super::layout::PaneParams) -> String: Full, counted => super::layout::separate_pane(&core, params)?,
    "resizeSplit" ResizeSplit (super::layout::ResizeSplit) -> (): Full => super::layout::resize_split(&core, params)?,
    "account": Watch => answer(core.account.shown()),
    "remoteImage": Watch => answer(super::remote_image::fetch(&core.remote_images, parse(params)?).await?),
    "accountPicture": Watch => answer(super::account::picture(&core, &parse::<super::account::AccountPictureParams>(params)?.hash).await),
    "accountSignIn": Owner => answer(super::account::sign_in(&core, client.peer.clone()).await?),
    "accountSignInFinish": Owner => answer(super::account::sign_in_finished(&core).await?),
    "accountSignInCancel": Owner => {
        super::account::cancel_sign_in(&core);
        answer(Value::Null)
    },
    "accountSignOut": Owner => answer(super::account::sign_out(&core, &client.peer).await?),
    "sshHosts": Watch, any_program => {
        let reading = core.clone();
        answer(blocking(move || Ok(super::ssh::hosts(&reading))).await?)
    },
    "removeTrigger": Full => answer(super::triggers::remove(&core, parse::<super::triggers::TriggerParams>(params)?.trigger)),
    "cancelResume" CancelResume (agent_accounts::ChatAccountsParams) -> (): Full => super::chat::reset::cancel_by_person(&core, &params.agent_id),
    "setAgentAccounts": Owner => {
        agent_accounts::set(&core, parse(params)?).await?;
        answer(())
    },
    "addAgentAccount": Owner, counted => answer(agent_accounts::add(parse(params)?).await?),
    "forgetAgentStatus": Owner => {
        agent_accounts::forget(&core, parse(params)?);
        answer(())
    },
    "markAgentSignedOut": Owner => {
        agent_accounts::mark_signed_out(&core, parse(params)?);
        answer(())
    },
    "saveAgentAccount": NoDevice => answer(agent_accounts::save(&core, parse(params)?).await?),
    "removeAgentAccount": NoDevice => answer(agent_accounts::remove(&core, parse(params)?).await?),
    "selectAgentAccount": NoDevice, counted => answer(agent_accounts::select(&core, parse(params)?).await?),
    "setAccountAutoSwitch": NoDevice, counted => {
        answer(agent_accounts::set_auto_switch(&core, parse(params)?).await?)
    },
    "retryFailed" RetryFailed (agent_accounts::ChatAccountsParams) -> (): Full => agent_accounts::retry_failed(&core, params)?,
    "discardFailed" DiscardFailed (agent_accounts::ChatAccountsParams) -> (): Full => agent_accounts::discard_failed(&core, params)?,
    "chatAccounts" ChatAccounts (agent_accounts::ChatAccountsParams) -> agent_accounts::ChatAccounts: Full => {
        agent_accounts::chat_accounts(&core, params)?
    },
    "moveChatAccount" MoveChatAccount (agent_accounts::MoveChatParams) -> (): Full => super::chat::move_to_account(&core, &params.agent_id, &params.account_id)?,
    "authenticateChat": NoDevice => {
        let params: agent_accounts::Authenticate = parse(params)?;
        super::chat::authenticate(&core, &params.agent_id, params.method_id).await?;
        answer(())
    },
    "shareChat" ShareChat (share::ShareChatParams) -> share::ShareView: Owner, counted => share::share_chat_for(&core, params)?,
    "stopShare": Owner => answer(share::stop(&core, &parse::<share::StopShareParams>(params)?.id)?),
    "shares": Full, any_program => answer(share::list(&core)),
    "pairing" RemotePairing (NoParams) -> remote::RemotePairing: Owner, any_program => remote::pairing(&core),
    "deviceChallenge": Owner => answer(super::account::devices::device_challenge(&core).await?),
    "approveHostSignIn": Owner => {
        answer(super::account::devices::approve_host_sign_in(&core, parse(params)?).await?)
    },
    "deviceJoinTicket": Owner => {
        answer(super::account::devices::device_join_ticket(&core, parse(params)?).await?)
    },
    "signHostRequest": NoDevice => core.remote.host_request_signature(&parse(params)?),
    "signShareViewer": NoDevice => core.remote.share_viewer_signature(parse(params)?),
    "answerPairing" AnswerPairing (remote::PairingAnswer) -> remote::RemotePairing: Owner, counted => remote::answer_pairing(&core, params)?,
    "changeDevice" ChangeDevice (remote::DeviceChange) -> remote::RemotePairing: Owner => remote::change_device(&core, params)?,
    "projectFiles": Full => {
        let params = parse(params)?;
        answer(blocking(move || super::project_files::read(params).map_err(CoreError::from)).await?)
    },
    "projectEntries" ProjectEntries (super::project_files::ProjectEntriesParams) -> Vec<crate::fuzzy::ProjectEntry>: Full => {
        blocking(move || super::project_files::entries(params).map_err(CoreError::from)).await?
    },
    "searchStream": Full, counted => answer(super::streams::search_stream(&core, client, parse(params)?)?),
    "searchReplace": Full, counted => {
        let params = parse(params)?;
        answer(blocking(move || super::search::replace(params).map_err(CoreError::from)).await?)
    },
    "fileWindow": Full => {
        let params = parse(params)?;
        answer(blocking(move || super::search::window(params).map_err(CoreError::from)).await?)
    },
    "streamCancel": Watch => {
        core.streams.cancel(client.id, parse::<super::streams::StreamParams>(params)?.stream);
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
    "setFaceStyle": Owner, counted => answer(super::faces::set_style_shown(&core, parse(params)?)?),
    "preferences" Preferences (NoParams) -> Preferences: Watch => core.preferences.get(),
    "mods": Watch => answer(super::mods::list(&core).await?),
    "modPage" ModPage (super::mods::ModParams) -> super::mods::ModPage: Watch => super::mods::page(&core, params).await?,
    "modRead": Owner => super::mods::read(&core, client, parse(params)?).await,
    "modWrite": Owner => super::mods::write(&core, client, parse(params)?).await,
    "allowMod" AllowMod (super::mods::AllowParams) -> (): Owner => super::mods::allow(&core, params).await?,
    "setModBadge" SetModBadge (super::mods::BadgeParams) -> (): Owner => super::mods::set_badge(&core, params)?,
    "pinMod" PinMod (super::mods::PinModParams) -> (): Owner => super::mods::pin(&core, params)?,
    "turnModOff" TurnModOff (super::mods::OffParams) -> (): Owner => super::mods::turn_off(&core, params)?,
    "removeMod" RemoveMod (super::mods::ModParams) -> (): Owner => super::mods::remove(&core, params).await?,
    "addExampleMod" AddExampleMod (super::mods::ModParams) -> (): Owner => super::mods::add_example(&core, params).await?,
    "stash": Owner => answer(super::stash::list(&core, parse_or_default(params)?).await?),
    "stashPrompt" StashPrompt (super::stash::StashPromptParams) -> super::stash::StashedPrompt: Owner, counted => {
        super::stash::stash_prompt(&core, params).await?
    },
    "seeStash" SeeStash (super::stash::SeeParams) -> crate::accounts::protocol::StashSeen: Owner => super::stash::see(&core, params).await?,
    "stashFile" StashFile (super::stash::StashItemParams) -> super::stash::StashKept: Owner, counted => super::stash::keep(&core, params).await?,
    "stashPicture": Full => answer(super::stash::picture(&core, parse(params)?).await?),
    "attachStash" AttachStash (super::stash::AttachStashParams) -> super::stash::AttachedStash: Owner, counted => {
        super::stash::attach(&core, params).await?
    },
    "removeStash" RemoveStash (super::stash::StashItemParams) -> (): Owner => super::stash::remove(&core, params).await?,
    "drafts": Watch => answer(core.drafts.get(parse_or_default::<super::drafts::DraftsParams>(params)?.keys.as_deref())),
    "setDraft" SetDraft (super::drafts::SetDraftParams) -> (): Owner => super::drafts::set(&core, params)?,
    "diagramLayout": Owner => answer(super::diagrams::layout(&core, parse(params)?)),
    "setDiagramLayout": Owner => super::diagrams::set_layout(&core, parse(params)?),
    "setPreference": Owner => {
        let params: super::preferences::SetPreferenceParams = parse(params)?;
        answer(super::preferences::set(&core, &params.key, params.value)?)
    },
    "setWorktreeDefault" SetWorktreeDefault (super::preferences::WorktreeDefaultParams) -> (): Owner => super::preferences::set_worktree_default(&core, params)?,
    "theme": Watch, any_program => answer(super::themes::palette_for(&core, parse_or_default(params)?)),
    "typeWhenReady": Full => super::session::type_when_asked(&core, client, parse(params)?),
    "promptDraft": Full => super::session::draft_at_prompt(&core, parse(params)?),
    "reconnectSsh": Full => super::session::reconnect(&core, parse(params)?).await,
    "newTerminal" NewTerminal (super::spare::NewTerminal) -> String: Full => super::spare::new_terminal(&core, params).await?,
    "showTerminals" ShowTerminals (NoParams) -> (): Full => super::spare::show_terminals(&core).await?,
    "stripShell" StripShell (sikemux_view::layout::At) -> sikemux_wire::protocol::SessionId: Full => super::spare::strip_shell(&core, params).await?,
    "openSshTerminal" OpenSshTerminal (super::spare::SshParams) -> String: Full, counted => super::spare::open_ssh_terminal(&core, params).await?,
    "takeSpareShell": NoDevice => answer(super::spare::take(&core, parse(params)?)),
    "themes": Watch => answer(super::themes::summaries(&core.preferences.get())),
    "themePalette": Watch => answer(super::themes::palette_of(parse::<super::themes::ThemePaletteParams>(params)?.theme)),
    "themeFromColours": Watch => answer(super::themes::built_from_colours(parse(params)?)?),
    "themeFromPixels": Watch, counted => answer(super::wallpaper::theme_from_pixels(parse(params)?)?),
    "codeHostPulls": Watch => answer(super::codehost::pulls(&core, client.id, parse(params)?).await),
    "codeHostTrackedContext" CodeHostTrackedContext (super::codehost::TrackedContextParams) -> plugins::PluginOutcome: Watch => {
        super::codehost::tracked_context(&core, params).await
    },
    "codeHostTracked" CodeHostTracked (super::codehost::CodeHostProjectParams) -> plugins::PluginOutcome: Watch => {
        super::codehost::tracked(&core, client.id, params).await
    },
    "hosts": Watch => answer(super::hosts::rows(&core)),
    "hostInstalledFrom": Owner => answer(super::hosts::installed_from(&core, parse(params)?)?),
    "installHost" InstallHost (super::hosts::install::InstallParams) -> Value: Owner => super::hosts::install::install_host(&core, params).await?,
    "hostSetup": Owner => answer(super::hosts::install::host_setup(&core, parse(params)?).await?),
    "linkHost": Owner, counted => answer(super::hosts::link_host(&core, parse(params)?).await?),
    "openOnHost" OpenOnHost (super::hosts::open::OpenOnParams) -> Value: Owner, counted => super::hosts::open::open_on(&core, params).await?,
    "sendSession" SendSession (super::hosts::transfer::SendParams) -> super::hosts::transfer::SessionSent: Owner, counted => super::hosts::transfer::send_session(&core, params).await?,
    "requireApproval" RequireApproval (NoParams) -> remote::ApprovalSetting: Owner => remote::ApprovalSetting {
        require_approval: core.remote.requires_approval(),
    },
    "setRequireApproval" SetRequireApproval (remote::ApprovalSetting) -> RemoteStatus: Owner => {
        remote::set_require_approval(&core, params)?
    },
    "questions" Questions (NoParams) -> Vec<AgentQuestion>: Watch, any_program => questions::views(&core),
    "answerQuestion" AnswerQuestion (questions::AnswerParams) -> (): Full, counted => questions::answer(&core, params).await?,
    "answerQuestions" AnswerQuestions (questions::AnswersParams) -> (): Full, counted => questions::answer_all(&core, params).await?,
    "allowShip" AllowShip (ship::HoldParams) -> (): Full => ship::allow(&core, params).await?,
    "refuseShip" RefuseShip (ship::HoldParams) -> (): Full => ship::refuse(&core, params).await?,
    "agentMessages": Watch, any_program => answer(messages::agent_messages(&core, parse(params)?).await?),
    "agentMessageRows" AgentMessageRows (messages::AgentMessagesParams) -> messages::AgentMessageRows: Watch, any_program => {
        messages::agent_message_rows(&core, params).await?
    },
    "boards" Boards (NoParams) -> Vec<board::ProjectBoard>: Watch, any_program => board::boards(&core),
    "board" Board (board::BoardParams) -> board::ProjectBoard: Watch, any_program => board::board(&core, params).await?,
    "boardAdd" BoardAdd (board::BoardAddParams) -> board::ProjectBoard: Full, counted => board::person_add(&core, params).await?,
    "boardHandOff" BoardHandOff (board::BoardHandOffParams) -> board::HandedOff: Full => board::hand_off(&core, params).await?,
    "boardResumeOwner" BoardResumeOwner (board::BoardCardParams) -> board::ProjectBoard: Full => board::person_resume(&core, params).await?,
    "boardHandOn" BoardHandOn (board::HandOnParams) -> board::ProjectBoard: Full => board::person_hand_on(&core, params).await?,
    "boardAddTicket" BoardAddTicket (board::BoardAddTicketParams) -> board::ProjectBoard: Full, counted => {
        board::tickets::person_add_ticket(&core, params).await?
    },
    "boardUpdate" BoardUpdate (board::BoardUpdateParams) -> board::ProjectBoard: Full, counted => board::person_update(&core, params).await?,
    "boardRemove" BoardRemove (board::BoardCardParams) -> board::ProjectBoard: Full => board::person_remove(&core, params).await?,
    "boardSettings" BoardSettings (board::BoardSettingsParams) -> board::ProjectBoard: Full => {
        board::person_settings(&core, params).await?
    },
    "boardAnswerProposal" BoardAnswerProposal (board::BoardProposalParams) -> board::ProjectBoard: Full => {
        board::person_answer_proposal(&core, params).await?
    },
    "goalSummary" GoalSummary (sikemux_view::slice::GoalSummaryParams) -> sikemux_view::slice::GoalSummaryView: Full => {
        goal_summary::read(&core, params).await?
    },
    "goals" Goals (NoParams) -> sikemux_view::goals::GoalsView: Watch, any_program => goals::view(&core),
    "setDesks" SetDesks (sikemux_view::goals::DesksParams) -> (): Owner => goals::set_desks(&core, client.id, params),
    "boardAssert" BoardAssert (board::goal::AssertParams) -> board::ProjectBoard: Full => {
        board::goal::person_assert(&core, params).await?
    },
    "boardDecide" BoardDecide (board::goal::DecideParams) -> board::ProjectBoard: Full => {
        board::goal::person_decide(&core, params).await?
    },
    "triage" Triage (super::triage::TriageParams) -> sikemux_view::start::Triage: Full => {
        super::triage::triage(&core, params).await?
    },
    "memory" Memory (memory::MemoryParams) -> MemoryView: Watch, any_program => memory::person_view(&core, client.peer.clone(), params).await?,
    "memoryAdd" MemoryAdd (memory::MemoryAddParams) -> MemoryView: Full => memory::person_add(&core, client.peer.clone(), params).await?,
    "memoryUpdate" MemoryUpdate (memory::MemoryUpdateParams) -> MemoryView: Full => memory::person_update(&core, client.peer.clone(), params).await?,
    "promoteRule" PromoteRule (memory::MemoryEntryParams) -> MemoryView: Owner => memory::promote(&core, client.peer.clone(), params).await?,
    "keepSeededRules" KeepSeededRules (memory::KeepSeededParams) -> MemoryView: Owner => memory::keep_seeded(&core, client.peer.clone(), params).await?,
    "seedRules" SeedRules (memory::MemoryParams) -> MemoryView: Owner => memory::seed_again(&core, client.peer.clone(), params).await?,
    "chatGoalTrack" ChatGoalTrack (board::chat_goal::ChatGoalParams) -> (): Full => {
        board::chat_goal::track(&core, params.agent_id).await?
    },
    "chatGoalUndo" ChatGoalUndo (board::chat_goal::ChatGoalParams) -> (): Full => {
        board::chat_goal::undo(&core, params.agent_id).await?
    },
    "chatGoalNotNow" ChatGoalNotNow (board::chat_goal::ChatGoalParams) -> (): Full => board::chat_goal::not_now(&core, &params.agent_id),
    "boardRetry" BoardRetry (board::BoardCardParams) -> board::ProjectBoard: Full => board::person_retry(&core, params).await?,
    "boardPass" BoardPass (board::BoardPassParams) -> board::ProjectBoard: Full => board::person_pass(&core, params).await?,
    "boardNote" BoardNote (board::BoardNoteParams) -> board::ProjectBoard: Full => board::person_note(&core, params).await?,
    "edits": Watch, any_program => answer(edits::read_edits(&core, parse(params)?).await?),
    "changeOwners" ChangeOwners (sikemux_view::owners::CheckoutParams) -> sikemux_view::owners::Owners: Watch, any_program => edits::change_owners(&core, params).await?,
    "commitStaged": Owner, counted => answer(edits::commit_staged(&core, parse(params)?).await?),
    "commitOwners" CommitOwners (sikemux_view::owners::CommitsParams) -> std::collections::BTreeMap<String, Vec<sikemux_view::owners::AgentOwner>>: Watch, any_program => who_wrote::call(&core, params).await?,
    "commitLines" CommitLines (sikemux_view::owners::CommitParams) -> sikemux_view::owners::CommitLines: Watch, any_program, counted => who_wrote::lines(&core, params).await?,
    "inbox": Watch, any_program => answer(inbox::read(&core).await?),
    "digest" Digest (inbox::DigestParams) -> sikemux_view::inbox::InboxDigest: Watch, any_program => inbox::read_digest(&core, params).await?,
    "ports": Owner => answer(super::ports::project_ports(&core, client.id, parse::<super::ports::PortsParams>(params)?.project).await?),
    "testResults": Owner => answer(super::tasks::module_tests(&core, parse::<super::tasks::ProjectParams>(params)?.project).await?),
    "projectConfig": Owner => answer(super::tasks::project_config(&core, parse::<super::tasks::ProjectParams>(params)?.project).await?),
    "startPreview": Owner, counted => answer(super::tasks::start_preview(&core, parse::<super::tasks::ProjectParams>(params)?.project).await?),
    "trustProject": Owner => answer(super::tasks::trust_project_for(&core, &client.peer, parse(params)?).await?),
    "tasks": Owner => answer(super::tasks::list(&core, parse::<super::tasks::ProjectParams>(params)?.project).await?),
    "taskRuns": Owner => answer(super::tasks::runs(&core)),
    "taskStart" TaskStart (super::tasks::TaskStartParams) -> Value: Owner, counted => super::tasks::start(&core, params).await?,
    "taskRerunFailed" TaskRerunFailed (super::tasks::RerunParams) -> Value: Owner, counted => super::tasks::rerun_failed(&core, params).await?,
    "taskStop": Owner => {
        super::harness::stop_runs(&core, parse(params)?).await;
        answer(())
    },
    "trustAsks": Owner => answer(super::tasks::asks(&core)),
    "answerTrust": Owner => answer(super::tasks::answer_trust_for(&core, &client.peer, parse(params)?)?),
    "addTask": Owner => answer(super::tasks::add(&core, parse(params)?, super::tasks::answered_by(&core, &client.peer)).await?),
    "removeTask" RemoveTask (super::tasks::RemoveTaskParams) -> Value: Owner => super::tasks::remove(&core, params).await?,
    "setCheck" SetCheck (super::tasks::SetCheckParams) -> Value: Owner => super::tasks::set_check(&core, params).await?,
    "agentModels": NoDevice => answer(super::chat::models_of(&core, &parse::<super::chat::AgentModelsParams>(params)?.agent)),
    "plugin": Owner, counted_by("plugin") => answer(plugins::window_call(&core, parse(params)?).await),
    "pluginManifests": Owner => answer(plugins::manifests(&core)),
    "releaseNotes": Watch => answer(super::about::release_notes(parse(params)?).await?),
    "releaseAvatars": Watch => answer(super::about::release_avatars(parse(params)?).await),
    "hostTools": Watch => answer(super::about::tools().await?),
    "whatsNew": Watch => answer(super::about::whats_new(&core)),
    "cliInstall": Watch => answer(super::cli_install::status().await?),
    "installCli": Owner, counted => answer(super::cli_install::install().await?),
    "renameAccountDevice" RenameAccountDevice (super::account::devices::DeviceParams) -> (): Owner => super::account::devices::rename_device(&core, params).await?,
    "removeAccountDevice" RemoveAccountDevice (super::account::devices::DeviceParams) -> (): Owner => super::account::devices::remove_device(&core, params).await?,
    "accountPhones": Owner => answer(super::account::devices::phones(&core).await?),
    "showFiles": Watch => {
        super::open_files::show(&core, client.id, parse(params)?)?;
        answer(())
    },
    "plugins": Watch => answer(plugins::listed(&core).await),
    "pluginSetDisabled": Owner => {
        plugins::set_disabled(&core, parse(params)?);
        answer(())
    },
    "pluginStream": Owner => answer(plugins::stream(&core, super::connection::connection_of(&core, client)?, parse(params)?)?),
    "pluginSignIn": Owner, any_program => {
        plugins::not_an_agent(client).await?;
        answer(plugins::sign_in(&core, super::connection::connection_of(&core, client)?, parse(params)?)?)
    },
    "signInCallback": Owner, any_program => {
        plugins::not_an_agent(client).await?;
        answer(plugins::sign_in_callback(&core, parse(params)?)?)
    },
    "files.list" FilesList (crate::files::FileListParams) -> Vec<crate::files::FileListing>: Full => edits::on_disk(move || crate::files::list(params)).await?,
    "files.kinds" FilesKinds (crate::files::FileKindsParams) -> Vec<Option<crate::files::FileKind>>: Full => edits::on_disk(move || crate::files::kinds(params)).await?,
    "files.read" FilesRead (crate::files::FileReadParams) -> crate::files::FilePiece: Full => edits::on_disk(move || crate::files::read(params, crate::files::PIECE_BYTES)).await?,
    "files.create" FilesCreate (crate::files::FileCreateParams) -> (): Full => edits::on_disk(move || crate::files::create(params)).await?,
    "files.rename" FilesRename (crate::files::FileRenameParams) -> (): Full => edits::on_disk(move || crate::files::rename(params)).await?,
    "files.delete" FilesDelete (crate::files::FilePathParams) -> (): Full => edits::on_disk(move || crate::files::delete(params)).await?,
    "files.readText" FilesReadText (crate::files::FileReadTextParams) -> String: Full => edits::on_disk(move || crate::files::read_text(params)).await?,
    "files.copyInto" FilesCopyInto (crate::files::FileCopyIntoParams) -> String: Full => edits::on_disk(move || crate::files::copy_into(params)).await?,
    "files.writeBytes" FilesWriteBytes (crate::files::FileWriteBytesParams) -> String: Full => edits::on_disk(move || crate::files::write_bytes(params)).await?,
    "files.preview" FilesPreview (crate::files::FilePreviewParams) -> crate::files::FilePreview: Full => edits::on_disk(move || crate::files::preview(params)).await?,
    "files.bytes" FilesBytes (crate::files::FileBytesParams) -> crate::files::FileBytes: Full => edits::on_disk(move || crate::files::bytes(params)).await?,
    "files.write" FilesWrite (crate::files::FileWriteParams) -> crate::files::FileWritten: Full => edits::write_file(&core, params).await?,
    "lsp.release": Full => super::lsp::call(&core, client.id, method, params).await,
    "lsp.claim": Full => super::lsp::call(&core, client.id, method, params).await,
    "lsp.languages": Full => super::lsp::call(&core, client.id, method, params).await,
    "lsp.install": Full => super::lsp::call(&core, client.id, method, params).await,
    "lsp.start": Full => super::lsp::call(&core, client.id, method, params).await,
    "lsp.stop": Full => super::lsp::call(&core, client.id, method, params).await,
    "lsp.open": Full => super::lsp::call(&core, client.id, method, params).await,
    "lsp.change": Full => super::lsp::call(&core, client.id, method, params).await,
    "lsp.changeIncremental": Full => super::lsp::call(&core, client.id, method, params).await,
    "lsp.save": Full => super::lsp::call(&core, client.id, method, params).await,
    "lsp.close": Full => super::lsp::call(&core, client.id, method, params).await,
    "lsp.locations": Full, counted => super::lsp::call(&core, client.id, method, params).await,
    "lsp.symbols": Full => super::lsp::call(&core, client.id, method, params).await,
    "lsp.diagnostics": Full => super::lsp::call(&core, client.id, method, params).await,
    "git.pullRequestUrl": Watch, any_program => answer(super::git::pull_request_url(&core, &client.peer, parse(params)?).await?),
}

pub(super) fn parse<T: for<'de> Deserialize<'de>>(params: Value) -> CoreResult<T> {
    serde_json::from_value(params)
        .map_err(|error| CoreError::from(format!("the call's params do not fit: {error}")))
}

/// Params a client may leave out, which then read as the defaults.
fn parse_or_default<T: for<'de> Deserialize<'de> + Default>(params: Value) -> CoreResult<T> {
    match params {
        Value::Null => Ok(T::default()),
        params => parse(params),
    }
}

/// A field the params name, null or not, as against one they leave out.
pub(super) fn given<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    field: D,
) -> Result<Option<Option<T>>, D::Error> {
    Option::deserialize(field).map(Some)
}

fn answer(result: impl Serialize) -> CoreResult<Value> {
    Ok(serde_json::to_value(result)?)
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

/// Where the person's own agent detection rules live.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestFolder {
    manifest_dir: Option<std::path::PathBuf>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentParams {
    pub(super) agent_id: String,
}
