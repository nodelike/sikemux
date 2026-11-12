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
    agent_accounts, agent_sign_in, attachments, board, edits, goal_summary, goals, inbox, lsp,
    memory, messages, model_providers, names, person_calls, plugins, questions, remote, saved,
    saved_sessions, share, ship, skill_library, who_wrote, Core, CoreError, CoreResult,
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
    "chatFile" ChatFile (attachments::ChatFileParams) -> attachments::ChatFile: Watch, any_program => attachments::chat_file(&core, &client.peer, params).await?,
    "keepPasted" KeepPasted (attachments::KeepPastedParams) -> attachments::Kept: Full => attachments::keep_pasted(&core, params).await?,
    "page" ReadPage (super::harness::pages::PageParams) -> super::harness::pages::PageHtml: Watch, any_program => super::harness::read_page(&core, params).await?,
    "nameAgent": NoDevice => answer(names::name_agent(&core, parse(params)?).await?),
    "appDeviceKey": NoDevice => answer(super::account::devices::app_device_key(parse(params)?).await?),
    "renameAgent" RenameAgent (names::RenameAgentParams) -> names::AgentName: Full, counted => names::rename_agent(&core, params).await?,
    "startSideChat" StartSideChat (super::chat::side::SideChatParams) -> super::chat::side::SideChat: Full, counted => super::chat::side::start(&core, params).await?,
    "promptChat" PromptChat (chat_calls::PromptChatParams) -> (): Full => chat_calls::prompt_chat(&core, params).await?,
    "closedAgents" ClosedAgents (names::ClosedAgentsParams) -> Vec<names::ClosedAgent>: Full => names::closed_agents(&core, params).await?,
    "agentNames": Watch, any_program => answer(names::agent_names(&core).await?),
    "agentCatalog" AgentCatalog (NoParams) -> Vec<CatalogAgent>: Watch, any_program => super::catalog::view(),
    "agentNameTheme" AgentNameTheme (NoParams) -> names::NameThemeParams: Full => names::name_theme(&core),
    "setAgentNameTheme" SetAgentNameTheme (names::NameThemeParams) -> names::NameThemeParams: Owner, counted => names::set_name_theme(&core, params),
    "hostLoad": Watch, any_program => answer(super::host_stats::load(&core)),
    "stopBuild" StopBuild (StopBuildParams) -> (): Owner => core.builds.stop(params.id).map_err(CoreError::from)?,
    "stopChatIfIdle": NoDevice => answer(json!({ "stopped": super::lifecycle::sleep(&core, &parse::<AgentParams>(params)?.agent_id) })),
    "switchAgentView" SwitchAgentView (super::agent_view::AgentViewParams) -> Value: Owner => super::agent_view::switch(&core, params).await?,
    "sleepAgent" SleepAgent (AgentParams) -> (): Owner, counted => super::lifecycle::sleep_agent(&core, &params.agent_id)?,
    "sleepIdleChats" SleepIdleChats (NoParams) -> Value: NoDevice => json!({ "slept": super::lifecycle::sleep_idle(&core) }),
    "startTerminalAgent": NoDevice, counted => super::launching::start_terminal_at(&core, parse(params)?).await,
    "commandUsed" CommandUsed (super::activity_log::CommandUsed) -> (): NoDevice => core.activity_log.command_used(&params.id),
    "composing" Composing (AgentParams) -> (): Owner => super::chat::composing(&core, &params.agent_id),
    "startChat" StartChat (super::launching::StartParams) -> (): NoDevice, counted => {
        super::launching::start_asked(&core, params).await?
    },
    "resumeSession" ResumeSession (super::workspace::SessionChoice) -> super::launching::SessionTaken: Full, counted => super::launching::resume_session(&core, params).await?,
    "reopenAgent" ReopenAgent (AgentParams) -> super::launching::SessionTaken: Full, counted => super::launching::reopen_agent(&core, params.agent_id).await?,
    "openChief" OpenChief (super::chief::OpenChiefParams) -> super::chief::OpenedChief: Full, counted => super::chief::open(&core, params).await?,
    "handOff" HandOff (super::chat::handoff::HandOffParams) -> super::chat::handoff::HandedOff: Full, counted => super::chat::handoff::hand_off(&core, params).await?,
    "dropHandoff" DropHandoff (super::chat::handoff::DropHandoffParams) -> (): Full => super::chat::handoff::drop_handoff(&core, params)?,
    "rotationBrief" RotationBrief (super::chat::rotation::RotationBriefParams) -> super::chat::rotation::RotationBriefRead: Watch => super::chat::rotation::read(&core, params)?,
    "activitySummary" ActivitySummary (super::activity_log::ActivitySummaryParams) -> super::activity_log::ActivitySummary: Watch, any_program => super::activity_log::summary_for(&core, params).await?,
    "setActivityPrice" SetActivityPrice (super::activity_log::ActivityPriceParams) -> (): Owner => super::activity_log::set_price_for(&core, params)?,
    "steerQueued" SteerQueued (chat_calls::QueuedParams) -> (): Full, counted => {
        super::chat::queue::steer(&core, &params.agent_id, params.ids, params.message).await?
    },
    "editMessage" EditMessage (chat_calls::EditParams) -> (): Full, counted => chat_calls::edit_message(&core, params).await?,
    "chatPage" ChatPage (chat_calls::ChatPageParams) -> sikemux_view::chat::ChatPage: Watch, any_program => {
        let reading = core.clone();
        blocking(move || super::chat::page(&reading, params)).await?
    },
    "chatFind" ChatFind (super::chat::find::FindParams) -> super::chat::find::FindAnswer: Watch, any_program, counted => {
        let (reading, client) = (core.clone(), client.id);
        blocking(move || super::chat::find(&reading, client, params)).await?
    },
    "chatToolDetail" ChatToolDetail (chat_calls::ToolDetailParams) -> ToolDetail: Watch, any_program => {
        super::chat::tool_detail(&core, &params.agent_id, &params.tool_call_id)?
    },
    "sendGuestMessage" SendGuestMessage (share::GuestMessageParams) -> (): Full, counted => share::send_guest(&core, params).await?,
    "dismissGuestMessage" DismissGuestMessage (share::GuestMessageParams) -> (): Full => share::dismiss_guest(&core, params),
    "dropQueued" DropQueued (chat_calls::QueuedParams) -> (): Full => {
        super::chat::queue::remove(&core, &params.agent_id, &params.ids)?
    },
    "agentSessions" AgentSessions (saved::SessionSearchParams) -> Vec<crate::sessions::search::SessionMatch>: Full, any_program => {
        saved::call(&core, params).await?
    },
    "workspace": Watch, any_program => answer(core.workspace_view_standing()),
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
    "updateStatus" UpdateStatus (NoParams) -> sikemux_view::updates::UpdateStatus: Watch, any_program => core.updates.status(),
    "updateCheck" UpdateCheck (NoParams) -> sikemux_view::updates::UpdateStatus: Owner => super::updates::check(&core).await?,
    "updateInstall" UpdateInstall (super::updates::UpdateInstallParams) -> sikemux_view::updates::UpdateStatus: Owner, counted => super::updates::install(&core, params)?,
    "updateCancel" UpdateCancel (NoParams) -> sikemux_view::updates::UpdateStatus: Owner => super::updates::cancel(&core),
    "updateHost": NoDevice, any_program => answer(super::updates::update_host(&core).await?),
    "setHostUpdates" SetHostUpdates (super::updates::HostUpdatesParams) -> sikemux_view::updates::UpdateStatus: Owner => super::updates::set_automatic(&core, params).await?,
    "runCustomCommand" RunCustomCommand (super::custom_command::RunCustomParams) -> super::custom_command::CommandRan: Owner, counted => super::custom_command::run_custom(&core, params).await?,
    "addProject" AddProject (workspace::AddProjectParams) -> sikemux_view::workspace::ProjectInfo: Owner => workspace::add_project(&core, params).await?,
    "newProject" NewProject (super::new_project::NewProjectParams) -> sikemux_view::workspace::ProjectInfo: Owner, counted => super::new_project::create(&core, params).await?,
    "cloneProject" CloneProject (super::new_project::CloneProjectParams) -> sikemux_view::workspace::ProjectInfo: Owner, counted => super::new_project::clone(&core, params).await?,
    "removeProject" RemoveProject (workspace::RemoveProjectParams) -> (): Owner => workspace::remove_project(&core, params)?,
    "updateProject" UpdateProject (workspace::UpdateProjectParams) -> sikemux_view::workspace::ProjectInfo: Owner => workspace::update_project(&core, params)?,
    "moveProject" MoveProject (workspace::MoveProjectParams) -> (): Owner => workspace::move_project(&core, params)?,
    "setSpaces" SetSpaces (workspace::SpacesParams) -> (): Owner, counted => workspace::set_spaces(&core, params)?,
    "setPinnedChats" SetPinnedChats (workspace::PinnedParams) -> (): Owner => workspace::set_pinned_chats(&core, params)?,
    "pinChat" PinChat (workspace::PinChatParams) -> (): Owner, counted => workspace::pin_chat(&core, params)?,
    "snoozeAgents" SnoozeAgents (super::settling::SnoozeParams) -> (): Full, counted => super::settling::snooze(&core, params)?,
    "snoozePresets" SnoozePresets (NoParams) -> Vec<super::settling::SnoozePreset>: Full => super::settling::presets(),
    "settleAgents" SettleAgents (super::settling::SettleParams) -> (): Full, counted => super::settling::settle(&core, params)?,
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
    "deleteSession": Owner => answer(saved_sessions::delete(&core, parse(params)?).await?),
    "renameRecent" RenameRecent (saved_sessions::RecentRenamed) -> (): Owner => saved_sessions::rename_recent(&core, params).await?,
    "deleteRecent" DeleteRecent (saved_sessions::RecentChat) -> (): Owner => saved_sessions::delete_recent(&core, params).await?,
    "relocateSession": Owner => answer(saved_sessions::relocate(parse(params)?).await?),
    "skillsOverview" SkillsOverview (NoParams) -> skill_library::SkillsOverview: Full, any_program => skill_library::overview(&core).await?,
    "skillsAdded" SkillsAdded (NoParams) -> Vec<skill_library::GlobalSkillAdded>: Full, any_program => skill_library::added(&core).await?,
    "createSkill" CreateSkill (skill_library::NewSkill) -> crate::skills::Skill: Owner, counted => skill_library::create(&core, params).await?,
    "installSkill" InstallSkill (skill_library::Source) -> crate::skills::Skill: Owner, counted => skill_library::install(&core, params).await?,
    "removeSkill" RemoveSkill (skill_library::Named) -> (): Owner => skill_library::remove(&core, params).await?,
    "adoptSkill" AdoptSkill (skill_library::Located) -> crate::skills::Skill: Owner, counted => skill_library::adopt(&core, params).await?,
    "agentSignIns" AgentSignIns (NoParams) -> Vec<agent_sign_in::Page>: Full, any_program => agent_sign_in::pages(),
    "agentSignIn" AgentSignIn (agent_sign_in::Account) -> (): Owner, counted => agent_sign_in::sign_in(&core, params).await?,
    "moveChatAfterSignIn" MoveChatAfterSignIn (agent_sign_in::AfterSignIn) -> (): Full => agent_sign_in::move_after_sign_in(&core, params)?,
    "signInLauncher" SignInLauncher (agent_sign_in::LauncherParams) -> (): Owner => agent_sign_in::sign_in_launcher(&core, params).await?,
    "agentSignInCode" AgentSignInCode (agent_sign_in::Code) -> (): Owner => agent_sign_in::sign_in_code(params).await?,
    "cancelAgentSignIn" CancelAgentSignIn (agent_sign_in::Account) -> (): Owner => agent_sign_in::cancel(params),
    "agentSignOut" AgentSignOut (agent_sign_in::Account) -> (): Owner => agent_sign_in::sign_out(params).await?,
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
    "agentAccounts" AgentAccounts (NoParams) -> crate::agent_accounts::Book: Full, any_program => core.workspaces.accounts().unwrap_or_default().with_built_ins(),
    "agentStatus": Full, any_program => answer(agent_accounts::status(&core, parse(params)?).await?),
    "installedAgents": Full, any_program => answer(agent_accounts::installed(&core).await?),
    "agentSetup" AgentSetup (NoParams) -> Vec<agent_accounts::Installed>: Full, any_program => agent_accounts::setup(&core).await?,
    "updateAgents" UpdateAgents (super::agent_updates::UpdateAgents) -> (): Full, any_program => super::agent_updates::update(&core, params).await?,
    "agentAvailability" AgentAvailability (agent_accounts::Account) -> agent_accounts::Availability: Full, any_program => {
        agent_accounts::availability(&core, params).await?
    },
    "agentAccountStatus" AgentAccountStatus (agent_accounts::Account) -> crate::agent_accounts::status::AccountStatus: Full, any_program => {
        agent_accounts::account_status(&core, params).await?
    },
    "agentUsage": Full, any_program => answer(agent_accounts::usage(&core, parse(params)?).await?),
    "agentUsages" AgentUsages (NoParams) -> Vec<agent_accounts::AccountUsage>: Watch, any_program => agent_accounts::usages(&core).await,
    "readAgentUsages" ReadAgentUsages (NoParams) -> Vec<agent_accounts::AccountUsage>: Full, any_program => agent_accounts::read_usages(&core).await,
    "useAgentReset" UseAgentReset (agent_accounts::UsingReset) -> agent_accounts::UsedReset: Full, counted => agent_accounts::use_reset(&core, params).await?,
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
    "remoteImage" RemoteImage (super::remote_image::ImageParams) -> super::remote_image::PictureData: Watch => super::remote_image::fetch(&core.remote_images, params).await?,
    "accountPicture" AccountPicture (super::account::AccountPictureParams) -> Option<super::remote_image::PictureData>: Watch => super::account::picture(&core, &params.hash).await,
    "accountSignIn" AccountSignIn (NoParams) -> super::account::SignInPage: Owner => super::account::sign_in(&core, client.peer.clone()).await?,
    "accountSignInFinish" AccountSignInFinish (NoParams) -> sikemux_view::device::AccountInfo: Owner => super::account::sign_in_finished(&core).await?,
    "accountSignInCancel" AccountSignInCancel (NoParams) -> (): Owner => super::account::cancel_sign_in(&core),
    "accountSignOut" AccountSignOut (NoParams) -> (): Owner => super::account::sign_out(&core, &client.peer).await?,
    "sshHosts" SshHosts (NoParams) -> Vec<super::ssh::SshHost>: Watch, any_program => {
        let reading = core.clone();
        blocking(move || Ok(super::ssh::hosts(&reading))).await?
    },
    "removeTrigger" RemoveTrigger (super::triggers::TriggerParams) -> bool: Full => super::triggers::remove(&core, params.trigger),
    "holdTrigger" HoldTrigger (super::triggers::HoldTriggerParams) -> bool: Full => super::triggers::hold(&core, params),
    "cancelResume" CancelResume (agent_accounts::ChatAccountsParams) -> (): Full => super::chat::reset::cancel_by_person(&core, &params.agent_id),
    "setAgentAccounts": Owner => {
        agent_accounts::set(&core, parse(params)?).await?;
        answer(())
    },
    "addAgentAccount" AddAgentAccount (agent_accounts::Adding) -> String: Owner, counted => agent_accounts::add(params).await?,
    "forgetAgentStatus" ForgetAgentStatus (agent_accounts::Forget) -> (): Owner => agent_accounts::forget(&core, params),
    "markAgentSignedOut": Owner => {
        agent_accounts::mark_signed_out(&core, parse(params)?);
        answer(())
    },
    "saveAgentAccount" SaveAgentAccount (agent_accounts::Saving) -> crate::agent_accounts::Book: NoDevice => agent_accounts::save(&core, params).await?,
    "removeAgentAccount" RemoveAgentAccount (agent_accounts::Removing) -> crate::agent_accounts::Book: NoDevice => agent_accounts::remove(&core, params).await?,
    "selectAgentAccount" SelectAgentAccount (agent_accounts::Choosing) -> crate::agent_accounts::Book: NoDevice, counted => agent_accounts::select(&core, params).await?,
    "pickAgentAccount" PickAgentAccount (agent_accounts::Choosing) -> agent_accounts::Picked: NoDevice, counted => agent_accounts::pick(&core, params).await?,
    "undoAgentAccountPick" UndoAgentAccountPick (agent_accounts::Unpicking) -> crate::agent_accounts::Book: NoDevice, counted => agent_accounts::unpick(&core, params).await?,
    "setSmartAccounts" SetSmartAccounts (agent_accounts::Smart) -> crate::agent_accounts::Book: NoDevice, counted => {
        agent_accounts::set_smart(&core, params).await?
    },
    "setAccountInProject" SetAccountInProject (agent_accounts::InProject) -> crate::agent_accounts::Book: NoDevice, counted => agent_accounts::set_in_project(&core, params).await?,
    "unpinAgentAccount" UnpinAgentAccount (agent_accounts::Unpinning) -> crate::agent_accounts::Book: NoDevice, counted => agent_accounts::unpin(&core, params).await?,
    "retryFailed" RetryFailed (agent_accounts::ChatAccountsParams) -> (): Full => agent_accounts::retry_failed(&core, params)?,
    "discardFailed" DiscardFailed (agent_accounts::ChatAccountsParams) -> (): Full => agent_accounts::discard_failed(&core, params)?,
    "chatAccounts" ChatAccounts (agent_accounts::ChatAccountsParams) -> agent_accounts::ChatAccounts: Full => {
        agent_accounts::chat_accounts(&core, params)?
    },
    "moveChatAccount" MoveChatAccount (agent_accounts::MoveChatParams) -> (): Full => super::chat::move_to_account(&core, &params.agent_id, &params.account_id).await?,
    "authenticateChat" AuthenticateChat (agent_accounts::Authenticate) -> (): NoDevice => {
        super::chat::authenticate(&core, &params.agent_id, params.method_id).await?
    },
    "shareChat" ShareChat (share::ShareChatParams) -> share::ShareView: Owner, counted => share::share_chat_for(&core, params)?,
    "stopShare" StopShare (share::StopShareParams) -> (): Owner => share::stop(&core, &params.id)?,
    "shares" Shares (NoParams) -> Vec<share::ShareView>: Full, any_program => share::list(&core),
    "pairing" RemotePairing (NoParams) -> remote::RemotePairing: Owner, any_program => remote::pairing(&core),
    "deviceChallenge": Owner => answer(super::account::devices::device_challenge(&core).await?),
    "approveHostSignIn": NoDevice => {
        answer(super::account::devices::approve_host_sign_in(&core, parse(params)?).await?)
    },
    "deviceJoinTicket": Owner => {
        answer(super::account::devices::device_join_ticket(&core, parse(params)?).await?)
    },
    "signHostRequest": NoDevice => core.remote.host_request_signature(&parse(params)?),
    "signShareViewer": NoDevice => core.remote.share_viewer_signature(parse(params)?),
    "answerPairing" AnswerPairing (remote::PairingAnswer) -> remote::RemotePairing: Owner, counted => remote::answer_pairing(&core, params)?,
    "changeDevice" ChangeDevice (remote::DeviceChange) -> remote::RemotePairing: Owner => remote::change_device(&core, params)?,
    "projectFiles" ProjectFiles (super::project_files::ProjectFilesParams) -> super::project_files::ProjectFiles: Full => {
        blocking(move || super::project_files::read(params).map_err(CoreError::from)).await?
    },
    "projectEntries" ProjectEntries (super::project_files::ProjectEntriesParams) -> Vec<crate::fuzzy::ProjectEntry>: Full => {
        blocking(move || super::project_files::entries(params).map_err(CoreError::from)).await?
    },
    "searchStream" SearchStream (super::streams::SearchStreamParams) -> Value: Full, counted => super::streams::search_stream(&core, client, params)?,
    "searchReplace" SearchReplace (super::search::ReplaceParams) -> super::search::ReplaceResults: Full, counted => {
        blocking(move || super::search::replace(params).map_err(CoreError::from)).await?
    },
    "fileWindow" FileWindow (super::search::WindowParams) -> super::search::FileWindow: Full => {
        blocking(move || super::search::window(params).map_err(CoreError::from)).await?
    },
    "streamCancel" StreamCancel (super::streams::StreamParams) -> (): Watch => core.streams.cancel(client.id, params.stream),
    "projectRoots" ProjectRoots (super::project_roots::ScanParams) -> Vec<super::project_roots::RootEntry>: Full => {
        blocking(move || super::project_roots::scan(params).map_err(CoreError::from)).await?
    },
    "resolvePath" ResolvePath (super::project_roots::ResolveParams) -> super::project_roots::Resolved: Full => {
        blocking(move || Ok(super::project_roots::resolve(params))).await?
    },
    "foundProjects" FoundProjects (NoParams) -> crate::sessions::found_projects::FoundProjects: Full => {
        workspace::launchers_known(&core).await;
        let accounts = core.workspaces.chat_accounts();
        blocking(move || Ok(crate::sessions::found_projects::found_projects(accounts, sikemux_process::clock::epoch_ms()))).await?
    },
    "sshConfigFile" SshConfigFile (NoParams) -> String: Full => blocking(|| super::ssh::config_file().map_err(CoreError::from)).await?,
    "faceStyle": Watch => answer(core.faces.style()),
    "setFaceStyle" SetFaceStyle (super::faces::FaceStyleChange) -> sikemux_view::device::FaceStyle: Owner, counted => super::faces::set_style_shown(&core, params)?,
    "preferences" Preferences (NoParams) -> Preferences: Watch => core.shown_preferences(),
    "mods" Mods (NoParams) -> super::mods::ModList: Watch => super::mods::list(&core).await?,
    "modPage" ModPage (super::mods::ModParams) -> super::mods::ModPage: Watch => super::mods::page(&core, params).await?,
    "modRead" ModRead (super::mods::BridgeParams) -> Value: Owner => super::mods::read(&core, client, params).await?,
    "modWrite" ModWrite (super::mods::BridgeParams) -> super::mods::BridgeWrite: Owner => super::mods::write(&core, client, params).await?,
    "allowMod" AllowMod (super::mods::AllowParams) -> (): Owner => super::mods::allow(&core, params).await?,
    "setModBadge" SetModBadge (super::mods::BadgeParams) -> (): Owner => super::mods::set_badge(&core, params)?,
    "pinMod" PinMod (super::mods::PinModParams) -> (): Owner => super::mods::pin(&core, params)?,
    "turnModOff" TurnModOff (super::mods::OffParams) -> (): Owner => super::mods::turn_off(&core, params)?,
    "removeMod" RemoveMod (super::mods::ModParams) -> (): Owner => super::mods::remove(&core, params).await?,
    "addExampleMod" AddExampleMod (super::mods::ModParams) -> (): Owner => super::mods::add_example(&core, params).await?,
    "stash" Stash (Option<super::stash::ListParams>) -> super::stash::ShownStash: Owner => super::stash::list(&core, params.unwrap_or_default()).await?,
    "stashPrompt" StashPrompt (super::stash::StashPromptParams) -> super::stash::StashedPrompt: Owner, counted => {
        super::stash::stash_prompt(&core, params).await?
    },
    "seeStash" SeeStash (super::stash::SeeParams) -> crate::accounts::protocol::StashSeen: Owner => super::stash::see(&core, params).await?,
    "stashFile" StashFile (super::stash::StashItemParams) -> super::stash::StashKept: Owner, counted => super::stash::keep(&core, params).await?,
    "stashPicture" StashPicture (super::stash::StashItemParams) -> super::remote_image::PictureData: Full => super::stash::picture(&core, params).await?,
    "attachStash" AttachStash (super::stash::AttachStashParams) -> super::stash::AttachedStash: Owner, counted => {
        super::stash::attach(&core, params).await?
    },
    "removeStash" RemoveStash (super::stash::StashItemParams) -> (): Owner => super::stash::remove(&core, params).await?,
    "vault" Vault (NoParams) -> super::vault::shown::ShownVault: Owner => super::vault::shown::vault(&core),
    "vaultAllow" VaultAllow (super::vault::shown::AllowParams) -> (): Owner, counted => super::vault::shown::allow(&core, params).await?,
    "vaultRevoke" VaultRevoke (super::vault::shown::RevokeParams) -> (): Owner, counted => super::vault::shown::revoke(&core, params).await?,
    "vaultRecoverySaved" VaultRecoverySaved (NoParams) -> (): Owner => super::vault::shown::recovery_saved(&core).await?,
    "vaultRecover" VaultRecover (super::vault::shown::RecoverParams) -> (): Owner => super::vault::shown::recover(&core, params).await?,
    "vaultUses" VaultUses (super::vault::shown::UsesParams) -> Vec<super::vault::shown::ShownUse>: Owner => super::vault::shown::uses(&core, params).await?,
    "drafts" Drafts (Option<super::drafts::DraftsParams>) -> std::collections::BTreeMap<String, super::drafts::Draft>: Watch => core.drafts.get(params.unwrap_or_default().keys.as_deref()),
    "setDraft" SetDraft (super::drafts::SetDraftParams) -> (): Owner => super::drafts::set(&core, params)?,
    "diagramLayout" DiagramLayout (super::diagrams::DiagramParams) -> super::diagrams::DiagramLayout: Owner => super::diagrams::layout(&core, params),
    "setDiagramLayout" SetDiagramLayout (super::diagrams::DiagramParams) -> Value: Owner => super::diagrams::set_layout(&core, params)?,
    "setPreference" SetPreference (sikemux_view::prefs::PreferenceChange) -> Preferences: Owner => super::preferences::set(&core, params)?,
    "setWorktreeDefault" SetWorktreeDefault (super::preferences::WorktreeDefaultParams) -> (): Owner => super::preferences::set_worktree_default(&core, params)?,
    "theme" Theme (Option<super::themes::ThemeParams>) -> sikemux_view::theme::Palette: Watch, any_program => super::themes::palette_for(&core, params.unwrap_or_default()),
    "typeWhenReady": Full => super::session::type_when_asked(&core, client, parse(params)?),
    "promptDraft": Full => super::session::draft_at_prompt(&core, parse(params)?),
    "reconnectSsh" ReconnectSsh (super::session::SessionParams) -> Value: Full => super::session::reconnect(&core, params).await?,
    "newTerminal" NewTerminal (super::spare::NewTerminal) -> String: Full => super::spare::new_terminal(&core, params).await?,
    "showTerminals" ShowTerminals (NoParams) -> (): Full => super::spare::show_terminals(&core).await?,
    "stripShell" StripShell (sikemux_view::layout::At) -> sikemux_wire::protocol::SessionId: Full => super::spare::strip_shell(&core, params).await?,
    "openSshTerminal" OpenSshTerminal (super::spare::SshParams) -> String: Full, counted => super::spare::open_ssh_terminal(&core, params).await?,
    "takeSpareShell": NoDevice => answer(super::spare::take(&core, parse(params)?)),
    "themes" Themes (NoParams) -> Vec<sikemux_view::theme::ThemeSummary>: Watch => super::themes::listed(&core),
    "themePalette" ThemePalette (super::themes::ThemePaletteParams) -> sikemux_view::theme::Palette: Watch => super::themes::palette_of(params.theme),
    "themeFromColours": Watch => answer(super::themes::built_from_colours(parse(params)?)?),
    "themeFromPixels" ThemeFromPixels (super::wallpaper::PixelsParams) -> sikemux_view::theme::Theme: Watch, counted => super::wallpaper::theme_from_pixels(params)?,
    "codeHostPulls" CodeHostPulls (super::codehost::PullsParams) -> plugins::PluginOutcome: Watch => super::codehost::pulls(&core, client.id, params).await,
    "codeHostTrackedContext" CodeHostTrackedContext (super::codehost::TrackedContextParams) -> plugins::PluginOutcome: Watch => {
        super::codehost::tracked_context(&core, params).await
    },
    "codeHostTracked" CodeHostTracked (super::codehost::CodeHostProjectParams) -> plugins::PluginOutcome: Watch => {
        super::codehost::tracked(&core, client.id, params).await
    },
    "hosts" Hosts (NoParams) -> Vec<super::hosts::HostRow>: Watch => super::hosts::rows(&core),
    "hostStats" HostStats (sikemux_view::host_stats::HostStatsParams) -> sikemux_view::host_stats::HostStats: Watch => super::host_stats::read(&core, params),
    "hostDiagnosis" HostDiagnosis (super::diagnosis::DiagnosisParams) -> sikemux_view::host_stats::HostDiagnosis: Watch => super::diagnosis::diagnose(&core, params),
    "appIcon" AppIcon (super::host_stats::apps::AppIconParams) -> Option<String>: Watch => super::host_stats::apps::icon(&core, params).await?,
    "hostInstalledFrom": Owner => answer(super::hosts::installed_from(&core, parse(params)?)?),
    "installHost" InstallHost (super::hosts::install::InstallParams) -> Value: Owner => super::hosts::install::install_host(&core, params).await?,
    "hostSetup" HostSetup (super::hosts::install::AliasParams) -> super::hosts::install::SetupEnded: Owner => super::hosts::install::host_setup(&core, params).await?,
    "dismissHostSetup" DismissHostSetup (super::hosts::install::AliasParams) -> (): Owner => super::hosts::install::dismiss_host_setup(&core, params)?,
    "linkHost": Owner, counted => answer(super::hosts::link_host(&core, parse(params)?).await?),
    "openOnHost" OpenOnHost (super::hosts::open::OpenOnParams) -> Value: Owner, counted => super::hosts::open::open_on(&core, params).await?,
    "moveChatToHost" MoveChatToHost (super::hosts::chat_move::MoveChatToHostParams) -> super::hosts::chat_move::ChatMoved: Owner, counted => super::hosts::chat_move::move_chat(&core, params).await?,
    "sendSession" SendSession (super::hosts::transfer::SendParams) -> super::hosts::transfer::SessionSent: Owner, counted => super::hosts::transfer::send_session(&core, params).await?,
    "requireApproval" RequireApproval (NoParams) -> remote::ApprovalSetting: Owner => remote::ApprovalSetting {
        require_approval: core.remote.requires_approval(),
    },
    "setRequireApproval" SetRequireApproval (remote::ApprovalSetting) -> RemoteStatus: Owner => {
        remote::set_require_approval(&core, params)?
    },
    "trustHeldRemote" TrustHeldRemote (NoParams) -> RemoteStatus: NoDevice => remote::trust_held(&core).await?,
    "forgetHeldRemote" ForgetHeldRemote (NoParams) -> RemoteStatus: NoDevice => remote::discard_held(&core)?,
    "questions" Questions (NoParams) -> Vec<AgentQuestion>: Watch, any_program => questions::views(&core),
    "answerQuestion" AnswerQuestion (questions::AnswerParams) -> (): Full, counted => {
        person_calls::answer_questions(&core, &client.peer, questions::AnswersParams { answers: vec![params] }).await?
    },
    "answerQuestions" AnswerQuestions (questions::AnswersParams) -> (): Full, counted => person_calls::answer_questions(&core, &client.peer, params).await?,
    "allowShip" AllowShip (ship::HoldParams) -> (): Full, counted => person_calls::answer_hold(&core, &client.peer, params, person_calls::HOLD_ALLOW).await?,
    "refuseShip" RefuseShip (ship::HoldParams) -> (): Full, counted => person_calls::answer_hold(&core, &client.peer, params, person_calls::HOLD_REFUSE).await?,
    "letShipGo" LetShipGo (ship::HoldParams) -> (): Full => person_calls::answer_hold(&core, &client.peer, params, person_calls::HOLD_ALWAYS).await?,
    "answerCall" AnswerCall (sikemux_view::calls::AnswerCallParams) -> (): Full, counted => person_calls::answer(&core, &client.peer, params).await?,
    "callSeen" CallSeen (sikemux_view::calls::CallParams) -> (): Watch => person_calls::seen(&core, &client.peer, params)?,
    "callNotMine" CallNotMine (sikemux_view::calls::CallParams) -> (): Full, counted => person_calls::not_mine(&core, &client.peer, params).await?,
    "places" PlacesRead (super::places::PlacesParams) -> sikemux_view::places::Places: Watch, any_program => super::places::read(&core, params).await?,
    "setPlaceLevel" SetPlaceLevel (super::places::PlaceLevelParams) -> sikemux_view::places::Places: Full, counted => super::places::set_level(&core, &client.peer, params).await?,
    "confirmPlaces" ConfirmPlaces (super::places::PlacesParams) -> sikemux_view::places::Places: Full, counted => super::places::confirm(&core, params).await?,
    "addPlace" AddPlace (super::places::AddPlaceParams) -> sikemux_view::places::Places: Full, counted => super::places::add(&core, &client.peer, params).await?,
    "countAsProduction" CountAsProduction (super::places::SeenParams) -> sikemux_view::places::Places: Full, counted => super::places::count_as_production(&core, &client.peer, params).await?,
    "placePin" PlacePin (sikemux_view::pins::PlacePinParams) -> sikemux_view::pins::PlacedPin: Full, counted => super::pins::place(&core, params).await?,
    "notePin" NotePin (sikemux_view::pins::PinNoteParams) -> (): Full, counted => super::pins::note(&core, params).await?,
    "pinLooksRight" PinLooksRight (sikemux_view::pins::PinParams) -> (): Full, counted => super::pins::looks_right(&core, params)?,
    "pinAfter" PinAfter (sikemux_view::pins::PinAfterParams) -> (): Full => super::pins::after(&core, params)?,
    "pinPicture" PinPicture (sikemux_view::pins::PinPictureParams) -> sikemux_view::pins::PinPicture: Watch => super::pins::picture(&core, params)?,
    "checkWebWrite" CheckWebWrite (ship::WebWriteParams) -> Option<String>: Owner => ship::check_web_write(&core, params).await?,
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
    "takeStep" TakeStep (super::turn_state::TakeStepParams) -> (): Full, counted_by("step") => super::turn_state::take_step(&core, params).await?,
    "turnPicture" TurnPicture (super::pictures::TurnPictureParams) -> super::pictures::TurnPictureData: Watch => super::pictures::picture(&core, params).await?,
    "landedMeans" LandedMeans (super::landing::LandedMeansParams) -> sikemux_view::turn::LandedMeans: Watch => super::landing::landed_means(&core, params.project).await?,
    "setLandedMeans" SetLandedMeans (super::landing::SetLandedMeansParams) -> sikemux_view::turn::LandedMeans: Full, counted => super::landing::set_landed_means(&core, params).await?,
    "boardSettings" BoardSettings (board::BoardSettingsParams) -> board::ProjectBoard: Full => {
        board::person_settings(&core, params).await?
    },
    "boardAnswerProposal" BoardAnswerProposal (board::BoardProposalParams) -> board::ProjectBoard: Full => {
        board::person_answer_proposal(&core, params).await?
    },
    "boardForgetInherited" BoardForgetInherited (board::ForgetInheritedParams) -> board::ProjectBoard: Full => {
        board::person_forget_inherited(&core, params).await?
    },
    "goalSummary" GoalSummary (sikemux_view::slice::GoalSummaryParams) -> sikemux_view::slice::GoalSummaryView: Full => {
        goal_summary::read(&core, params).await?
    },
    "boardAuditEvery" BoardAuditEvery (board::audit::AuditEveryParams) -> board::ProjectBoard: Full => {
        board::audit::person_audit_every(&core, params).await?
    },
    "boardAuditNow" BoardAuditNow (board::audit::AuditNowParams) -> board::ProjectBoard: Full => {
        board::audit::person_audit_now(&core, params).await?
    },
    "goalReports" GoalReports (board::report::GoalReportsParams) -> Vec<sikemux_view::slice::GoalReport>: Watch, any_program => {
        board::report::reports(&core, params)?
    },
    "boardReportEvery" BoardReportEvery (board::report::ReportEveryParams) -> board::ProjectBoard: Full => {
        board::report::person_report_every(&core, params).await?
    },
    "goals" Goals (NoParams) -> sikemux_view::goals::GoalsView: Watch, any_program => goals::view(&core),
    "setDesks" SetDesks (sikemux_view::goals::DesksParams) -> (): Owner => goals::set_desks(&core, client.id, params),
    "boardAssert" BoardAssert (board::goal::AssertParams) -> board::ProjectBoard: Full => {
        board::goal::person_assert(&core, params).await?
    },
    "boardAnswerCommand" BoardAnswerCommand (board::goal::AnswerCommandParams) -> board::ProjectBoard: Full => {
        board::goal::person_answer_command(&core, params).await?
    },
    "overruleDecision" OverruleDecision (board::goal::OverruleParams) -> board::ProjectBoard: Full, counted => {
        board::goal::overrule(&core, params).await?
    },
    "boardDecide" BoardDecide (board::goal::DecideParams) -> board::ProjectBoard: Full => {
        board::goal::person_decide(&core, params).await?
    },
    "triage" Triage (super::triage::TriageParams) -> sikemux_view::start::Triage: Full => {
        super::triage::triage(&core, params).await?
    },
    "playbook" Playbook (super::playbook::PlaybookParams) -> sikemux_view::playbook::Playbook: Watch => super::playbook::read(&core, client.peer.clone(), params).await?,
    "memory" Memory (memory::MemoryParams) -> MemoryView: Watch, any_program => memory::person_view(&core, client.peer.clone(), params).await?,
    "memoryAdd" MemoryAdd (memory::MemoryAddParams) -> MemoryView: Full => memory::person_add(&core, client.peer.clone(), params).await?,
    "memoryUpdate" MemoryUpdate (memory::MemoryUpdateParams) -> MemoryView: Full => memory::person_update(&core, client.peer.clone(), params).await?,
    "promoteRule" PromoteRule (memory::MemoryEntryParams) -> MemoryView: Owner => memory::promote(&core, client.peer.clone(), params).await?,
    "keepSeededRules" KeepSeededRules (memory::KeepSeededParams) -> MemoryView: Owner => memory::keep_seeded(&core, client.peer.clone(), params).await?,
    "seedRules" SeedRules (memory::MemoryParams) -> MemoryView: Owner => memory::seed_again(&core, client.peer.clone(), params).await?,
    "chatGoalUndo" ChatGoalUndo (board::chat_goal::ChatGoalParams) -> (): Full => {
        board::chat_goal::undo(&core, params.agent_id).await?
    },
    "boardRetry" BoardRetry (board::BoardCardParams) -> board::ProjectBoard: Full => board::person_retry(&core, params).await?,
    "boardPass" BoardPass (board::BoardPassParams) -> board::ProjectBoard: Full, counted => board::person_pass(&core, params).await?,
    "boardNote" BoardNote (board::BoardNoteParams) -> board::ProjectBoard: Full => board::person_note(&core, params).await?,
    "edits": Watch, any_program => answer(edits::read_edits(&core, parse(params)?).await?),
    "changeOwners" ChangeOwners (sikemux_view::owners::CheckoutParams) -> sikemux_view::owners::Owners: Watch, any_program => edits::change_owners(&core, params).await?,
    "commitStaged" CommitStaged (edits::CommitParams) -> String: Owner, counted => edits::commit_staged(&core, params).await?,
    "commitOwners" CommitOwners (sikemux_view::owners::CommitsParams) -> std::collections::BTreeMap<String, Vec<sikemux_view::owners::AgentOwner>>: Watch, any_program => who_wrote::call(&core, params).await?,
    "commitLines" CommitLines (sikemux_view::owners::CommitParams) -> sikemux_view::owners::CommitLines: Watch, any_program, counted => who_wrote::lines(&core, params).await?,
    "inbox": Watch, any_program => answer(inbox::read(&core).await?),
    "digest" Digest (inbox::DigestParams) -> sikemux_view::inbox::InboxDigest: Watch, any_program => inbox::read_digest(&core, params).await?,
    "ports" Ports (super::ports::PortsParams) -> Vec<super::ports::ProjectPort>: Owner => super::ports::project_ports(&core, client.id, params.project).await?,
    "testResults": Owner => answer(super::tasks::module_tests(&core, parse::<super::tasks::ProjectParams>(params)?.project).await?),
    "projectConfig" ProjectConfig (super::tasks::ProjectParams) -> super::tasks::ProjectConfigRead: Owner => super::tasks::project_config(&core, params.project).await?,
    "startPreview" StartPreview (super::tasks::ProjectParams) -> String: Owner, counted => super::tasks::start_preview(&core, params.project).await?,
    "trustProject" TrustProject (super::tasks::TrustProjectParams) -> (): Owner => super::tasks::trust_project_for(&core, &client.peer, params).await?,
    "tasks" Tasks (super::tasks::ProjectParams) -> super::tasks::TaskList: Owner => super::tasks::list(&core, params.project).await?,
    "taskRuns" TaskRuns (NoParams) -> Vec<super::tasks::RunView>: Owner => super::tasks::runs(&core),
    "taskStart" TaskStart (super::tasks::TaskStartParams) -> super::tasks::TaskStarted: Owner, counted => super::tasks::start(&core, params).await?,
    "taskRerunFailed" TaskRerunFailed (super::tasks::RerunParams) -> super::tasks::RerunStarted: Owner, counted => super::tasks::rerun_failed(&core, params).await?,
    "taskStop" TaskStop (super::harness::RunSelector) -> (): Owner => super::harness::stop_runs(&core, params).await,
    "trustAsks" TrustAsks (NoParams) -> Vec<super::tasks::TrustQuestion>: Owner => super::tasks::asks(&core),
    "answerTrust" AnswerTrust (super::tasks::TrustAnswerParams) -> (): Owner => super::tasks::answer_trust_for(&core, &client.peer, params)?,
    "addTask" AddTask (super::tasks::AddParams) -> super::tasks::TaskAdded: Owner => super::tasks::add(&core, params, super::tasks::answered_by(&core, &client.peer)).await?,
    "removeTask" RemoveTask (super::tasks::RemoveTaskParams) -> Value: Owner => super::tasks::remove(&core, params).await?,
    "setCheck" SetCheck (super::tasks::SetCheckParams) -> Value: Owner => super::tasks::set_check(&core, params).await?,
    "smartPick" SmartPick (super::launching::SmartPickParams) -> Option<sikemux_view::chat::AccountMove>: Watch => super::launching::smart_pick(&core, params).await?,
    "agentModels": NoDevice => answer(super::chat::models_of(&core, &parse::<super::chat::AgentModelsParams>(params)?.agent)),
    "plugin" Plugin (plugins::PluginCall) -> plugins::PluginOutcome: Owner, counted_by("plugin") => plugins::window_call(&core, params).await,
    "pluginManifests": Owner => answer(plugins::manifests(&core)),
    "releaseNotes" ReleaseNotes (super::about::NotesParams) -> crate::releases::ReleaseNotes: Watch => super::about::release_notes(params).await?,
    "releaseAvatars" ReleaseAvatars (super::about::AvatarsParams) -> std::collections::HashMap<String, String>: Watch => super::about::release_avatars(params).await,
    "hostTools" HostTools (NoParams) -> super::about::Tools: Watch => super::about::tools().await?,
    "whatsNew" WhatsNew (NoParams) -> super::about::WhatsNew: Watch => super::about::whats_new(&core),
    "cliInstall" CliInstall (NoParams) -> super::cli_install::CliInstallStatus: Watch => super::cli_install::status().await?,
    "installCli" InstallCli (NoParams) -> super::cli_install::CliInstallStatus: Owner, counted => super::cli_install::install().await?,
    "renameAccountDevice" RenameAccountDevice (super::account::devices::DeviceParams) -> (): Owner => super::account::devices::rename_device(&core, params).await?,
    "removeAccountDevice" RemoveAccountDevice (super::account::devices::DeviceParams) -> (): Owner => super::account::devices::remove_device(&core, params).await?,
    "accountPhones" AccountPhones (NoParams) -> Vec<crate::accounts::protocol::Device>: Owner => super::account::devices::phones(&core).await?,
    "showFiles" ShowFiles (super::open_files::ShowFiles) -> (): Watch => super::open_files::show(&core, client.id, params)?,
    "plugins" Plugins (NoParams) -> Vec<sikemux_view::plugins::Listed>: Watch => plugins::listed(&core).await,
    "pluginSetDisabled" PluginSetDisabled (plugins::Disabled) -> (): Owner => plugins::set_disabled(&core, params),
    "pluginStream" PluginStream (plugins::StreamCall) -> Value: Owner => plugins::stream(&core, super::connection::connection_of(&core, client)?, params)?,
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
    "lsp.release" LspRelease (NoParams) -> (): Full => super::lsp::release(client.id),
    "lsp.claim" LspClaim (lsp::LspClaim) -> Vec<String>: Full => lsp::claim(client.id, params),
    "lsp.languages" LspLanguages (NoParams) -> Vec<String>: Full => lsp::languages().await?,
    "lsp.install" LspInstall (lsp::LspLanguage) -> String: Full => lsp::install(params).await?,
    "lsp.start" LspStart (lsp::LspServer) -> (): Full => lsp::start(&core, params).await?,
    "lsp.stop" LspStop (lsp::LspProject) -> (): Full => lsp::stop(params).await?,
    "lsp.open" LspOpen (lsp::LspOpen) -> lsp::LspOpened: Full => lsp::open(client.id, params).await?,
    "lsp.change" LspChange (lsp::LspChange) -> (): Full => lsp::change(params).await?,
    "lsp.changeIncremental" LspChangeIncremental (lsp::LspIncrementalChange) -> (): Full => lsp::change_incremental(params).await?,
    "lsp.save" LspSave (lsp::LspSave) -> (): Full => lsp::save(params).await?,
    "lsp.close" LspClose (lsp::LspDocument) -> (): Full => lsp::close(client.id, params).await?,
    "lsp.locations" LspLocations (lsp::LspLocations) -> Vec<lsp::LspLocation>: Full, counted => lsp::locations(params).await?,
    "lsp.symbols" LspSymbols (lsp::LspDocument) -> Vec<lsp::LspDocumentSymbol>: Full => lsp::symbols(params).await?,
    "lsp.diagnostics" LspDiagnostics (lsp::LspPath) -> Vec<lsp::LspDiagnosticsPayload>: Full => lsp::diagnostics_of(params),
    "git.pullRequestUrl" GitPullRequestUrl (super::git::PullRequestUrlParams) -> String: Watch, any_program => super::git::pull_request_url(&core, &client.peer, params).await?,
}

pub(super) fn parse<T: for<'de> Deserialize<'de>>(params: Value) -> CoreResult<T> {
    serde_json::from_value(params)
        .map_err(|error| CoreError::from(format!("the call's params do not fit: {error}")))
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

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StopBuildParams {
    pub id: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentParams {
    pub agent_id: String,
}
