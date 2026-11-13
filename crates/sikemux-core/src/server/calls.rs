//! The core's methods a client reaches by name through `Request::Call`. A
//! device gains a method here without its native code changing, so one a
//! released app calls is never removed or renamed.

use crate::server::device_view::DeviceViewCore;
use crate::server::parts::Parts;
use crate::server::ServerCore;
pub use sikemux_calls::params::{AgentParams, GoFirstParams, NoParams, StopBuildParams};
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
/// a client in Rust. A method whose marker and types sikemux-calls declares is named there,
/// as `"m" = Marker`. The `git.*` methods have the same rows in `git.rs`.
macro_rules! calls {
    ($core:ident, $client:ident, $method:ident, $params:ident;
     $($name:literal $(= $moved:ident)? $($marker:ident ($takes:ty) -> $gives:ty)?: $needs:ident $(, $flag:ident $(($arg:expr))?)* => $run:expr,)*) => {
        /// The methods that name their params and answer, one type each, for a client
        /// in Rust to call them typed.
        pub mod call {
            pub use sikemux_calls::Call;
            $($(pub use sikemux_calls::call::$moved;)?)*

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
            let (there, here) = super::hosts::route::elsewhere(&$core, $method, $params);
            let mut answered = Value::Null;
            for (host, params) in there {
                answered = super::hosts::route::act(&$core, &host, $method, params).await?;
            }
            let Some($params) = here else {
                return Ok(answered);
            };
            // Each method's future lives on the heap, so the dispatch's own frame never carries a
            // method's whole chain on a worker thread's stack.
            match $method {
                $($name => Box::pin(async move { calls!(@run $params, $run $(, $takes, $gives)? $(, <call::$moved as call::Call>::Params, <call::$moved as call::Call>::Answer)?) }).await,)*
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
    "chatFile" = ChatFile: Watch, any_program => attachments::chat_file(&core, &client.peer, params).await?,
    "keepPasted" = KeepPasted: Full => attachments::keep_pasted(&core, params).await?,
    "page" = ReadPage: Watch, any_program => super::harness::read_page(&core, params).await?,
    "mcpApp" = ReadMcpApp: Watch, any_program => super::mcp_apps::read(&core, params).await?,
    "mcpAppRequest" = McpAppRequest: Full => super::mcp_apps::request(&core, params).await?,
    "nameAgent": NoDevice => answer(names::name_agent(&core, parse(params)?).await?),
    "appDeviceKey": NoDevice => answer(super::account::devices::app_device_key(parse(params)?).await?),
    "renameAgent" = RenameAgent: Full, counted => names::rename_agent(&core, params).await?,
    "startSideChat" = StartSideChat: Full, counted => super::chat::side::start(&core, params).await?,
    "promptChat" = PromptChat: Full => chat_calls::prompt_chat(&core, params).await?,
    "onScreen" = OnScreen: Full => super::seen::on_screen(&core, client.id, params.agent_ids, params.focused, false),
    "closedAgents" = ClosedAgents: Full => names::closed_agents(&core, params).await?,
    "agentNames": Watch, any_program => answer(names::agent_names(&core).await?),
    "agentCatalog" = AgentCatalog: Watch, any_program => super::catalog::view(),
    "agentNameTheme" = AgentNameTheme: Full => names::name_theme(&core),
    "setAgentNameTheme" = SetAgentNameTheme: Owner, counted => names::set_name_theme(&core, params),
    "hostLoad": Watch, any_program => answer(super::host_stats::load(&core)),
    "stopBuild" = StopBuild: Owner => core.line().stop(params.id).map_err(CoreError::from)?,
    "goFirst" = GoFirst: Owner => core.line().go_first(&core, params.id).map_err(CoreError::from)?,
    "stopChatIfIdle": NoDevice => answer(json!({ "stopped": super::lifecycle::sleep(&core, &parse::<AgentParams>(params)?.agent_id) })),
    "switchAgentView" = SwitchAgentView: Owner => super::agent_view::switch(&core, params).await?,
    "sleepAgent" = SleepAgent: Owner, counted => super::lifecycle::sleep_agent(&core, &params.agent_id)?,
    "sleepIdleChats" = SleepIdleChats: NoDevice => json!({ "slept": super::lifecycle::sleep_idle(&core) }),
    "startTerminalAgent": NoDevice, counted => super::launching::start_terminal_at(&core, parse(params)?).await,
    "commandUsed" = CommandUsed: NoDevice => core.activity_log().command_used(&params.id),
    "composing" = Composing: Owner => super::chat::composing(&core, &params.agent_id),
    "startChat" = StartChat: NoDevice, counted => {
        super::launching::start_asked(&core, params).await?
    },
    "resumeSession" = ResumeSession: Full, counted => super::launching::resume_session(&core, params).await?,
    "reopenAgent" = ReopenAgent: Full, counted => super::launching::reopen_agent(&core, params.agent_id).await?,
    "openChief" = OpenChief: Full, counted => super::chief::open(&core, params).await?,
    "handOff" = HandOff: Full, counted => super::chat::handoff::hand_off(&core, params).await?,
    "dropHandoff" = DropHandoff: Full => super::chat::handoff::drop_handoff(&core, params)?,
    "keepHistory" = KeepHistory: Full => super::chat::cap::keep_history(&core, params)?,
    "rotationBrief" = RotationBrief: Watch => super::chat::rotation::read(&core, params)?,
    "activitySummary" = ActivitySummary: Watch, any_program => super::activity_log::summary_for(&core, params).await?,
    "setActivityPrice" = SetActivityPrice: Owner => super::activity_log::set_price_for(&core, params)?,
    "steerQueued" = SteerQueued: Full, counted => {
        super::chat::queue::steer(&core, &params.agent_id, params.ids, params.message).await?
    },
    "editMessage" = EditMessage: Full, counted => chat_calls::edit_message(&core, params).await?,
    "chatPage" = ChatPage: Watch, any_program => {
        let reading = core.clone();
        blocking(move || super::chat::page(&reading, params)).await?
    },
    "chatFind" = ChatFind: Watch, any_program, counted => {
        let (reading, client) = (core.clone(), client.id);
        blocking(move || super::chat::find(&reading, client, params)).await?
    },
    "chatToolDetail" = ChatToolDetail: Watch, any_program => {
        super::chat::tool_detail(&core, &params.agent_id, &params.tool_call_id)?
    },
    "sendGuestMessage" = SendGuestMessage: Full, counted => share::send_guest(&core, params).await?,
    "dismissGuestMessage" = DismissGuestMessage: Full => share::dismiss_guest(&core, params),
    "chatLeftOut" = ChatLeftOut: Watch, counted => {
        let reading = core.clone();
        blocking(move || super::chat::left_out(&reading, &params.agent_id, &params.message_id)).await?
    },
    "dropQueued" = DropQueued: Full => {
        super::chat::queue::remove(&core, &params.agent_id, &params.ids)?
    },
    "agentSessions" = AgentSessions: Full, any_program => {
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
    "updateStatus" = UpdateStatus: Watch, any_program => core.updates().status(),
    "updateCheck" = UpdateCheck: Owner => super::updates::check(&core).await?,
    "updateInstall" = UpdateInstall: Owner, counted => super::updates::install(&core, params)?,
    "updateCancel" = UpdateCancel: Owner => super::updates::cancel(&core),
    "updateHost": NoDevice, any_program => answer(super::updates::update_host(&core).await?),
    "setHostUpdates" = SetHostUpdates: Owner => super::updates::set_automatic(&core, params).await?,
    "runCustomCommand" = RunCustomCommand: Owner, counted => super::custom_command::run_custom(&core, params).await?,
    "addProject" = AddProject: Owner => workspace::add_project(&core, params).await?,
    "newProject" = NewProject: Owner, counted => super::new_project::create(&core, params).await?,
    "cloneProject" = CloneProject: Owner, counted => super::new_project::clone(&core, params).await?,
    "removeProject" = RemoveProject: Owner => workspace::remove_project(&core, params)?,
    "updateProject" = UpdateProject: Owner => workspace::update_project(&core, params)?,
    "moveProject" = MoveProject: Owner => workspace::move_project(&core, params)?,
    "setSpaces" = SetSpaces: Owner, counted => workspace::set_spaces(&core, params)?,
    "setPinnedChats" = SetPinnedChats: Owner => workspace::set_pinned_chats(&core, params)?,
    "pinChat" = PinChat: Owner, counted => workspace::pin_chat(&core, params)?,
    "snoozeAgents" = SnoozeAgents: Full, counted => super::settling::snooze(&core, params)?,
    "snoozePresets" = SnoozePresets: Full => super::settling::presets(),
    "settleAgents" = SettleAgents: Full, counted => super::settling::settle(&core, params)?,
    "recentChats" = RecentChats: Watch => workspace::recent_chats(&core, params).await?,
    "setAgentDefaults" = SetAgentDefaults: Owner => workspace::set_defaults(&core, params).await?,
    "createAgent": Owner, counted => answer(workspace::create_agent(&core, parse(params)?)?),
    "updateAgent" = UpdateAgent: Owner => workspace::update_agent(&core, params)?,
    "moveAgentToProject" = MoveAgentToProject: Owner, counted => super::move_agent::move_agent(&core, params).await?,
    "moveAgentToFolder" = MoveAgentToFolder: Owner => super::move_agent::move_to_folder(&core, params).await?,
    "closeAgents" = CloseAgents: Owner => workspace::close_agents(&core, params)?,
    "closeChat" = CloseChat: Full => workspace::close_agent(&core, params.agent_id)?,
    "createAgentWorktree" = CreateAgentWorktree: Owner, counted => {
        super::worktree::agent::for_agent(&core, params).await?
    },
    "agentReview" = AgentReview: Watch, counted => super::worktree::removal::review(&core, params).await?,
    "worktreeMerged" = WorktreeMerged: Watch => super::worktree::removal::merged(&core, params).await?,
    "worktreeRemoval" = WorktreeRemoval: Owner => super::worktree::removal::removal(&core, params).await?,
    "removeAgentWorktree" = RemoveAgentWorktree: Owner, counted => super::worktree::removal::remove(&core, params).await?,
    "checkout" = Checkout: Watch => super::ports::checkout::checkout(&core, params).await?,
    "agentWorktreeAt": Full => answer(super::worktree::agent::at(parse(params)?).await?),
    "savedSessions": Full, any_program => answer(saved_sessions::list(parse(params)?).await?),
    "recentSessions": Full, any_program => answer(saved_sessions::recent(parse(params)?).await?),
    "renameSession": Owner => answer(saved_sessions::rename(parse(params)?).await?),
    "deleteSession": Owner => answer(saved_sessions::delete(&core, parse(params)?).await?),
    "renameRecent" = RenameRecent: Owner => saved_sessions::rename_recent(&core, params).await?,
    "deleteRecent" = DeleteRecent: Owner => saved_sessions::delete_recent(&core, params).await?,
    "relocateSession": Owner => answer(saved_sessions::relocate(parse(params)?).await?),
    "skillsOverview" = SkillsOverview: Full, any_program => skill_library::overview(&core).await?,
    "skillsAdded" = SkillsAdded: Full, any_program => skill_library::added(&core).await?,
    "createSkill" = CreateSkill: Owner, counted => skill_library::create(&core, params).await?,
    "installSkill" = InstallSkill: Owner, counted => skill_library::install(&core, params).await?,
    "removeSkill" = RemoveSkill: Owner => skill_library::remove(&core, params).await?,
    "adoptSkill" = AdoptSkill: Owner, counted => skill_library::adopt(&core, params).await?,
    "agentSignIns" = AgentSignIns: Full, any_program => agent_sign_in::pages(),
    "agentSignIn" = AgentSignIn: Owner, counted => agent_sign_in::sign_in(&core, params).await?,
    "moveChatAfterSignIn" = MoveChatAfterSignIn: Full => agent_sign_in::move_after_sign_in(&core, params)?,
    "signInLauncher" = SignInLauncher: Owner => agent_sign_in::sign_in_launcher(&core, params).await?,
    "agentSignInCode" = AgentSignInCode: Owner => agent_sign_in::sign_in_code(params).await?,
    "cancelAgentSignIn" = CancelAgentSignIn: Owner => agent_sign_in::cancel(params),
    "agentSignOut" = AgentSignOut: Owner => agent_sign_in::sign_out(params).await?,
    "modelProviders" = ModelProviders: Full, any_program => model_providers::list(&core).await?,
    "modelProviderModels" = ModelProviderModels: Full, any_program => model_providers::models(&core, params).await?,
    "modelHelper" = ModelHelper: Full, any_program => model_providers::helper(&core).await?,
    "modelProviderCheck" = ModelProviderCheck: Owner => model_providers::check(&core, params).await?,
    "modelProviderConnect" = ModelProviderConnect: Owner, counted => model_providers::connect(&core, params).await?,
    "modelProviderDisconnect" = ModelProviderDisconnect: Owner => model_providers::disconnect(&core, params).await?,
    "modelProviderRead" = ModelProviderRead: Owner => model_providers::read(&core, params).await?,
    "modelProviderAdd" = ModelProviderAdd: Owner => model_providers::add(&core, params).await?,
    "modelProviderRemove" = ModelProviderRemove: Owner => model_providers::remove(&core, params).await?,
    "setModelHelper" = SetModelHelper: Owner => model_providers::set_helper(&core, params).await?,
    "agentAccounts" = AgentAccounts: Full, any_program => core.workspaces().accounts().unwrap_or_default().with_built_ins(),
    "agentStatus": Full, any_program => answer(agent_accounts::status(&core, parse(params)?).await?),
    "installedAgents": Full, any_program => answer(agent_accounts::installed(&core).await?),
    "firstRunStep" = FirstRunStep: Full => super::usage::reach(&core, params),
    "agentSetup" = AgentSetup: Full, any_program => agent_accounts::setup(&core).await?,
    "updateAgents" = UpdateAgents: Full, any_program => super::agent_updates::update(&core, params).await?,
    "agentAvailability" = AgentAvailability: Full, any_program => {
        agent_accounts::availability(&core, params).await?
    },
    "agentAccountStatus" = AgentAccountStatus: Full, any_program => {
        agent_accounts::account_status(&core, params).await?
    },
    "agentUsage": Full, any_program => answer(agent_accounts::usage(&core, parse(params)?).await?),
    "agentUsages" = AgentUsages: Watch, any_program => agent_accounts::usages(&core).await,
    "readAgentUsages" = ReadAgentUsages: Full, any_program => agent_accounts::read_usages(&core).await,
    "useAgentReset" = UseAgentReset: Full, counted => agent_accounts::use_reset(&core, params).await?,
    "openTab" = OpenTab: Full => super::layout::open_tab(&core, params)?,
    "keepTab" = KeepTab: Full => super::layout::keep_tab(&core, params)?,
    "closeTab" = CloseTab: Full => super::layout::close_tab(&core, params)?,
    "activateTab" = ActivateTab: Full => super::layout::activate_tab(&core, params)?,
    "moveTab" = MoveTab: Full => super::layout::move_tab(&core, params)?,
    "closeTabs" = CloseTabs: Full => super::layout::close_tabs(&core, params)?,
    "renameTab" = RenameTab: Full => super::layout::rename_tab(&core, params)?,
    "duplicateTab" = DuplicateTab: Full, counted => super::layout::duplicate_tab(&core, params).await?,
    "activateProject" = ActivateProject: Full => super::layout::activate_project(&core, params)?,
    "openPaths" = OpenPaths: Owner => super::layout::open_paths(&core, params).await?,
    "chooseHost" = ChooseHost: Full => super::layout::choose_host(&core, params).await?,
    "splitPane" = SplitPane: Full, counted => super::layout::split_pane(&core, params)?,
    "closePane" = ClosePane: Full => super::layout::close_pane(&core, params)?,
    "separatePane" = SeparatePane: Full, counted => super::layout::separate_pane(&core, params)?,
    "resizeSplit" = ResizeSplit: Full => super::layout::resize_split(&core, params)?,
    "account": Watch => answer(core.account().shown()),
    "remoteImage" = RemoteImage: Watch => super::remote_image::fetch(core.remote_images(), params).await?,
    "accountPicture" = AccountPicture: Watch => super::account::picture(&core, &params.hash).await,
    "accountSignIn" = AccountSignIn: Owner => super::account::sign_in(&core, client.peer.clone()).await?,
    "accountSignInFinish" = AccountSignInFinish: Owner => super::account::sign_in_finished(&core).await?,
    "accountSignInCancel" = AccountSignInCancel: Owner => super::account::cancel_sign_in(&core),
    "accountSignOut" = AccountSignOut: Owner => super::account::sign_out(&core, &client.peer).await?,
    "sshHosts" = SshHosts: Watch, any_program => {
        let reading = core.clone();
        blocking(move || Ok(super::ssh::hosts(&reading))).await?
    },
    "removeTrigger" = RemoveTrigger: Full => super::triggers::remove(&core, params.trigger),
    "holdTrigger" = HoldTrigger: Full => super::triggers::hold(&core, params),
    "cancelResume" = CancelResume: Full => super::chat::reset::cancel_by_person(&core, &params.agent_id),
    "setAgentAccounts": Owner => {
        agent_accounts::set(&core, parse(params)?).await?;
        answer(())
    },
    "addAgentAccount" = AddAgentAccount: Owner, counted => agent_accounts::add(params).await?,
    "forgetAgentStatus" = ForgetAgentStatus: Owner => agent_accounts::forget(&core, params),
    "markAgentSignedOut": Owner => {
        agent_accounts::mark_signed_out(&core, parse(params)?);
        answer(())
    },
    "saveAgentAccount" = SaveAgentAccount: NoDevice => agent_accounts::save(&core, params).await?,
    "removeAgentAccount" = RemoveAgentAccount: NoDevice => agent_accounts::remove(&core, params).await?,
    "selectAgentAccount" = SelectAgentAccount: NoDevice, counted => agent_accounts::select(&core, params).await?,
    "pickAgentAccount" = PickAgentAccount: NoDevice, counted => agent_accounts::pick(&core, params).await?,
    "undoAgentAccountPick" = UndoAgentAccountPick: NoDevice, counted => agent_accounts::unpick(&core, params).await?,
    "setSmartAccounts" = SetSmartAccounts: NoDevice, counted => {
        agent_accounts::set_smart(&core, params).await?
    },
    "setAccountInProject" = SetAccountInProject: NoDevice, counted => agent_accounts::set_in_project(&core, params).await?,
    "retryFailed" = RetryFailed: Full => agent_accounts::retry_failed(&core, params)?,
    "discardFailed" = DiscardFailed: Full => agent_accounts::discard_failed(&core, params)?,
    "chatAccounts" = ChatAccounts: Full => {
        agent_accounts::chat_accounts(&core, params)?
    },
    "moveChatAccount" = MoveChatAccount: Full => super::chat::move_to_account(&core, &params.agent_id, &params.account_id).await?,
    "authenticateChat" = AuthenticateChat: NoDevice => {
        super::chat::authenticate(&core, &params.agent_id, params.method_id).await?
    },
    "shareChat" = ShareChat: Owner, counted => share::share_chat_for(&core, params)?,
    "stopShare" = StopShare: Owner => share::stop(&core, &params.id)?,
    "shares" = Shares: Full, any_program => share::list(&core),
    "pairing" = RemotePairing: Owner, any_program => remote::pairing(&core),
    "deviceChallenge": Owner => answer(super::account::devices::device_challenge(&core).await?),
    "approveHostSignIn": NoDevice => {
        answer(super::account::devices::approve_host_sign_in(&core, parse(params)?).await?)
    },
    "deviceJoinTicket": Owner => {
        answer(super::account::devices::device_join_ticket(&core, parse(params)?).await?)
    },
    "signHostRequest": NoDevice => core.remote().host_request_signature(&parse(params)?),
    "signShareViewer": NoDevice => core.remote().share_viewer_signature(parse(params)?),
    "answerPairing" = AnswerPairing: Owner, counted => remote::answer_pairing(&core, params)?,
    "changeDevice" = ChangeDevice: Owner => remote::change_device(&core, params)?,
    "projectFiles" = ProjectFiles: Full => {
        blocking(move || super::project_files::read(params).map_err(CoreError::from)).await?
    },
    "projectEntries" = ProjectEntries: Full => {
        blocking(move || super::project_files::entries(params).map_err(CoreError::from)).await?
    },
    "searchStream" = SearchStream: Full, counted => super::streams::search_stream(&core, client, params)?,
    "searchReplace" = SearchReplace: Full, counted => {
        blocking(move || super::search::replace(params).map_err(CoreError::from)).await?
    },
    "fileWindow" = FileWindow: Full => {
        blocking(move || super::search::window(params).map_err(CoreError::from)).await?
    },
    "streamCancel" = StreamCancel: Watch => core.streams().cancel(client.id, params.stream),
    "projectRoots" = ProjectRoots: Full => {
        blocking(move || super::project_roots::scan(params).map_err(CoreError::from)).await?
    },
    "resolvePath" = ResolvePath: Full => {
        blocking(move || Ok(super::project_roots::resolve(params))).await?
    },
    "foundProjects" = FoundProjects: Full => {
        workspace::launchers_known(&core).await;
        let accounts = core.workspaces().chat_accounts();
        blocking(move || Ok(crate::sessions::found_projects::found_projects(accounts, sikemux_process::clock::epoch_ms()))).await?
    },
    "sshConfigFile" = SshConfigFile: Full => blocking(|| super::ssh::config_file().map_err(CoreError::from)).await?,
    "faceStyle": Watch => answer(core.faces().style()),
    "setFaceStyle" = SetFaceStyle: Owner, counted => super::faces::set_style_shown(&core, params)?,
    "preferences" = Preferences: Watch => core.shown_preferences(),
    "mods" = Mods: Watch => super::mods::list(&core).await?,
    "modPage" = ModPage: Watch => super::mods::page(&core, params).await?,
    "modRead" = ModRead: Owner => super::mods::read(&core, client, params).await?,
    "modWrite" = ModWrite: Owner => super::mods::write(&core, client, params).await?,
    "allowMod" = AllowMod: Owner => super::mods::allow(&core, params).await?,
    "setModBadge" = SetModBadge: Owner => super::mods::set_badge(&core, params)?,
    "pinMod" = PinMod: Owner => super::mods::pin(&core, params)?,
    "turnModOff" = TurnModOff: Owner => super::mods::turn_off(&core, params)?,
    "removeMod" = RemoveMod: Owner => super::mods::remove(&core, params).await?,
    "addExampleMod" = AddExampleMod: Owner => super::mods::add_example(&core, params).await?,
    "stash" = Stash: Owner => super::stash::list(&core, params.unwrap_or_default()).await?,
    "stashPrompt" = StashPrompt: Owner, counted => {
        super::stash::stash_prompt(&core, params).await?
    },
    "seeStash" = SeeStash: Owner => super::stash::see(&core, params).await?,
    "stashFile" = StashFile: Owner, counted => super::stash::keep(&core, params).await?,
    "stashPicture" = StashPicture: Full => super::stash::picture(&core, params).await?,
    "attachStash" = AttachStash: Owner, counted => {
        super::stash::attach(&core, params).await?
    },
    "removeStash" = RemoveStash: Owner => super::stash::remove(&core, params).await?,
    "vault" = Vault: Owner => super::vault::shown::vault(&core),
    "vaultAllow" = VaultAllow: Owner, counted => super::vault::shown::allow(&core, params).await?,
    "vaultRevoke" = VaultRevoke: Owner, counted => super::vault::shown::revoke(&core, params).await?,
    "vaultRecoverySaved" = VaultRecoverySaved: Owner => super::vault::shown::recovery_saved(&core).await?,
    "vaultRecover" = VaultRecover: Owner => super::vault::shown::recover(&core, params).await?,
    "vaultUses" = VaultUses: Owner => super::vault::shown::uses(&core, params).await?,
    "drafts" = Drafts: Watch => core.drafts().get(params.unwrap_or_default().keys.as_deref()),
    "setDraft" = SetDraft: Owner => super::drafts::set(&core, params)?,
    "diagramLayout" = DiagramLayout: Owner => super::diagrams::layout(&core, params),
    "setDiagramLayout" = SetDiagramLayout: Owner => super::diagrams::set_layout(&core, params)?,
    "setPreference" = SetPreference: Owner => super::preferences::set(&core, params)?,
    "setWorktreeDefault" = SetWorktreeDefault: Owner => super::preferences::set_worktree_default(&core, params)?,
    "theme" = Theme: Watch, any_program => super::themes::palette_for(&core, params.unwrap_or_default()),
    "typeWhenReady": Full => super::session::type_when_asked(&core, client, parse(params)?),
    "promptDraft": Full => super::session::draft_at_prompt(&core, parse(params)?),
    "reconnectSsh" = ReconnectSsh: Full => super::session::reconnect(&core, params).await?,
    "newTerminal" = NewTerminal: Full => super::spare::new_terminal(&core, params).await?,
    "showTerminals" = ShowTerminals: Full => super::spare::show_terminals(&core).await?,
    "stripShell" = StripShell: Full => super::spare::strip_shell(&core, params).await?,
    "openSshTerminal" = OpenSshTerminal: Full, counted => super::spare::open_ssh_terminal(&core, params).await?,
    "takeSpareShell": NoDevice => answer(super::spare::take(&core, parse(params)?)),
    "themes" = Themes: Watch => super::themes::listed(&core),
    "themePalette" = ThemePalette: Watch => super::themes::palette_of(params.theme),
    "themeFromColours": Watch => answer(super::themes::built_from_colours(parse(params)?)?),
    "themeFromPixels" = ThemeFromPixels: Watch, counted => super::wallpaper::theme_from_pixels(params)?,
    "codeHostPulls" = CodeHostPulls: Watch => super::codehost::pulls(&core, client.id, params).await,
    "codeHostTrackedContext" = CodeHostTrackedContext: Watch => {
        super::codehost::tracked_context(&core, params).await
    },
    "codeHostTracked" = CodeHostTracked: Watch => {
        super::codehost::tracked(&core, client.id, params).await
    },
    "outsideAccess" = OutsideAccess: NoDevice => core.outside().view(),
    "answerOutside" = AnswerOutside: NoDevice, counted => super::outside::answer(&core, params)?,
    "changeOutside" = ChangeOutside: NoDevice, counted => super::outside::change(&core, params)?,
    "hosts" = Hosts: Watch => super::hosts::rows(&core),
    "hostStats" = HostStats: Watch => super::host_stats::read(&core, params),
    "hostDiagnosis" = HostDiagnosis: Watch => super::diagnosis::diagnose(&core, params),
    "appIcon" = AppIcon: Watch => super::host_stats::apps::icon(&core, params).await?,
    "hostInstalledFrom": Owner => answer(super::hosts::installed_from(&core, parse(params)?)?),
    "installHost" = InstallHost: Owner => super::hosts::install::install_host(&core, params).await?,
    "hostSetup" = HostSetup: Owner => super::hosts::install::host_setup(&core, params).await?,
    "dismissHostSetup" = DismissHostSetup: Owner => super::hosts::install::dismiss_host_setup(&core, params)?,
    "linkHost": Owner, counted => answer(super::hosts::link_host(&core, parse(params)?).await?),
    "openOnHost" = OpenOnHost: Owner, counted => super::hosts::open::open_on(&core, params).await?,
    "moveChatToHost" = MoveChatToHost: Owner, counted => super::hosts::chat_move::move_chat(&core, params).await?,
    "sendSession" = SendSession: Owner, counted => super::hosts::transfer::send_session(&core, params).await?,
    "requireApproval" = RequireApproval: Owner => remote::ApprovalSetting {
        require_approval: core.remote().requires_approval(),
    },
    "setRequireApproval" = SetRequireApproval: Owner => {
        remote::set_require_approval(&core, params)?
    },
    "trustHeldRemote" = TrustHeldRemote: NoDevice => remote::trust_held(&core).await?,
    "forgetHeldRemote" = ForgetHeldRemote: NoDevice => remote::discard_held(&core)?,
    "questions" = Questions: Watch, any_program => questions::views(&core),
    "answerQuestion" = AnswerQuestion: Full, counted => {
        person_calls::answer_questions(&core, &client.peer, questions::AnswersParams { answers: vec![params] }).await?
    },
    "answerQuestions" = AnswerQuestions: Full, counted => person_calls::answer_questions(&core, &client.peer, params).await?,
    "allowShip" = AllowShip: Full, counted => person_calls::answer_hold(&core, &client.peer, params, person_calls::HOLD_ALLOW).await?,
    "refuseShip" = RefuseShip: Full, counted => person_calls::answer_hold(&core, &client.peer, params, person_calls::HOLD_REFUSE).await?,
    "letShipGo" = LetShipGo: Full => person_calls::answer_hold(&core, &client.peer, params, person_calls::HOLD_ALWAYS).await?,
    "answerCall" = AnswerCall: Full, counted => person_calls::answer(&core, &client.peer, params).await?,
    "liveActivity" = LiveActivity: Watch => super::notify::live_activity(&core, &client.peer, params)?,
    "callSeen" = CallSeen: Watch => person_calls::seen(&core, &client.peer, params)?,
    "callNotMine" = CallNotMine: Full, counted => person_calls::not_mine(&core, &client.peer, params).await?,
    "places" = PlacesRead: Watch, any_program => super::places::read(&core, params).await?,
    "setPlaceLevel" = SetPlaceLevel: Full, counted => super::places::set_level(&core, &client.peer, params).await?,
    "confirmPlaces" = ConfirmPlaces: Full, counted => super::places::confirm(&core, params).await?,
    "addPlace" = AddPlace: Full, counted => super::places::add(&core, &client.peer, params).await?,
    "countAsProduction" = CountAsProduction: Full, counted => super::places::count_as_production(&core, &client.peer, params).await?,
    "placePin" = PlacePin: Full, counted => super::pins::place(&core, params).await?,
    "notePin" = NotePin: Full, counted => super::pins::note(&core, params).await?,
    "pinLooksRight" = PinLooksRight: Full, counted => super::pins::looks_right(&core, params)?,
    "pinAfter" = PinAfter: Full => super::pins::after(&core, params)?,
    "pinPicture" = PinPicture: Watch => super::pins::picture(&core, params)?,
    "checkWebWrite" = CheckWebWrite: Owner => ship::check_web_write(&core, params).await?,
    "agentMessages": Watch, any_program => answer(messages::agent_messages(&core, parse(params)?).await?),
    "agentMessageRows" = AgentMessageRows: Watch, any_program => {
        messages::agent_message_rows(&core, params).await?
    },
    "boards" = Boards: Watch, any_program => board::boards(&core),
    "board" = Board: Watch, any_program => board::board(&core, params).await?,
    "boardAdd" = BoardAdd: Full, counted => board::person_add(&core, params).await?,
    "boardHandOff" = BoardHandOff: Full => board::hand_off(&core, params).await?,
    "boardResumeOwner" = BoardResumeOwner: Full => board::person_resume(&core, params).await?,
    "boardHandOn" = BoardHandOn: Full => board::person_hand_on(&core, params).await?,
    "boardAddTicket" = BoardAddTicket: Full, counted => {
        board::tickets::person_add_ticket(&core, params).await?
    },
    "boardUpdate" = BoardUpdate: Full, counted => board::person_update(&core, params).await?,
    "boardRemove" = BoardRemove: Full => board::person_remove(&core, params).await?,
    "takeStep" = TakeStep: Full, counted_by("step") => super::turn_state::take_step(&core, params).await?,
    "turnPicture" = TurnPicture: Watch => super::pictures::picture(&core, params).await?,
    "landedMeans" = LandedMeans: Watch => super::landing::landed_means(&core, params.project).await?,
    "setLandedMeans" = SetLandedMeans: Full, counted => super::landing::set_landed_means(&core, params).await?,
    "boardSettings" = BoardSettings: Full => {
        board::person_settings(&core, params).await?
    },
    "boardAnswerProposal" = BoardAnswerProposal: Full => {
        board::person_answer_proposal(&core, params).await?
    },
    "boardForgetInherited" = BoardForgetInherited: Full => {
        board::person_forget_inherited(&core, params).await?
    },
    "goalSummary" = GoalSummary: Full => {
        goal_summary::read(&core, params).await?
    },
    "boardAuditEvery" = BoardAuditEvery: Full => {
        board::audit::person_audit_every(&core, params).await?
    },
    "boardAuditNow" = BoardAuditNow: Full => {
        board::audit::person_audit_now(&core, params).await?
    },
    "goalReports" = GoalReports: Watch, any_program => {
        board::report::reports(&core, params)?
    },
    "boardReportEvery" = BoardReportEvery: Full => {
        board::report::person_report_every(&core, params).await?
    },
    "goals" = Goals: Watch, any_program => goals::view(&core),
    "setDesks" = SetDesks: Owner => goals::set_desks(&core, client.id, params),
    "boardAssert" = BoardAssert: Full => {
        board::goal::person_assert(&core, params).await?
    },
    "boardAnswerCommand" = BoardAnswerCommand: Full => {
        board::goal::person_answer_command(&core, params).await?
    },
    "overruleDecision" = OverruleDecision: Full, counted => {
        board::goal::overrule(&core, params).await?
    },
    "undoOverrule" = UndoOverrule: Full, counted => {
        board::goal::undo_overrule(&core, params).await?
    },
    "boardDecide" = BoardDecide: Full => {
        board::goal::person_decide(&core, params).await?
    },
    "triage" = Triage: Full => {
        super::triage::triage(&core, params).await?
    },
    "playbook" = Playbook: Watch => super::playbook::read(&core, client.peer.clone(), params).await?,
    "memory" = Memory: Watch, any_program => memory::person_view(&core, client.peer.clone(), params).await?,
    "memoryAdd" = MemoryAdd: Full => memory::person_add(&core, client.peer.clone(), params).await?,
    "memoryUpdate" = MemoryUpdate: Full => memory::person_update(&core, client.peer.clone(), params).await?,
    "promoteRule" = PromoteRule: Owner => memory::promote(&core, client.peer.clone(), params).await?,
    "keepSeededRules" = KeepSeededRules: Owner => memory::keep_seeded(&core, client.peer.clone(), params).await?,
    "seedRules" = SeedRules: Owner => memory::seed_again(&core, client.peer.clone(), params).await?,
    "chatGoalUndo" = ChatGoalUndo: Full => {
        board::chat_goal::undo(&core, params.agent_id).await?
    },
    "boardRetry" = BoardRetry: Full => board::person_retry(&core, params).await?,
    "boardPass" = BoardPass: Full, counted => board::person_pass(&core, params).await?,
    "boardNote" = BoardNote: Full => board::person_note(&core, params).await?,
    "edits": Watch, any_program => answer(edits::read_edits(&core, parse(params)?).await?),
    "changeOwners" = ChangeOwners: Watch, any_program => edits::change_owners(&core, params).await?,
    "commitStaged" = CommitStaged: Owner, counted => edits::commit_staged(&core, params).await?,
    "commitOwners" = CommitOwners: Watch, any_program => who_wrote::call(&core, params).await?,
    "commitLines" = CommitLines: Watch, any_program, counted => who_wrote::lines(&core, params).await?,
    "inbox": Watch, any_program => answer(inbox::read(&core).await?),
    "digest" = Digest: Watch, any_program => inbox::read_digest(&core, params).await?,
    "ports" = Ports: Owner => super::ports::project_ports(&core, client.id, params.project).await?,
    "testResults": Owner => answer(super::tasks::module_tests(&core, parse::<super::tasks::ProjectParams>(params)?.project).await?),
    "projectConfig" = ProjectConfig: Owner => super::tasks::project_config(&core, params.project).await?,
    "startPreview" = StartPreview: Owner, counted => super::tasks::start_preview(&core, params.project).await?,
    "trustProject" = TrustProject: Owner => super::tasks::trust_project_for(&core, &client.peer, params).await?,
    "tasks" = Tasks: Owner => super::tasks::list(&core, params.project).await?,
    "taskRuns" = TaskRuns: Owner => super::tasks::runs(&core),
    "setupState" = SetupState: Watch => super::setup_check::state(&core, params).await?,
    "setupCheck" = SetupCheck: Owner, counted => super::setup_check::check(&core, &params.project, None).await?,
    "taskStart" = TaskStart: Owner, counted => super::tasks::start(&core, params).await?,
    "taskRerunFailed" = TaskRerunFailed: Owner, counted => super::tasks::rerun_failed(&core, params).await?,
    "taskStop" = TaskStop: Owner => super::harness::stop_runs(&core, params).await,
    "trustAsks" = TrustAsks: Owner => super::tasks::asks(&core),
    "answerTrust" = AnswerTrust: Owner => super::tasks::answer_trust_for(&core, &client.peer, params)?,
    "addTask" = AddTask: Owner => super::tasks::add(&core, params, super::tasks::answered_by(&core, &client.peer)).await?,
    "removeTask" = RemoveTask: Owner => super::tasks::remove(&core, params).await?,
    "setCheck" = SetCheck: Owner => super::tasks::set_check(&core, params).await?,
    "smartPick" = SmartPick: Watch => super::launching::smart_pick(&core, params).await?,
    "agentModels": NoDevice => answer(super::chat::models_of(&core, &parse::<super::chat::AgentModelsParams>(params)?.agent)),
    "plugin" = Plugin: Owner, counted_by("plugin") => plugins::window_call(&core, params).await,
    "pluginManifests": Owner => answer(plugins::manifests(&core)),
    "releaseNotes" = ReleaseNotes: Watch => super::about::release_notes(params).await?,
    "releaseAvatars" = ReleaseAvatars: Watch => super::about::release_avatars(params).await,
    "hostTools" = HostTools: Watch => super::about::tools().await?,
    "whatsNew" = WhatsNew: Watch => super::about::whats_new(&core),
    "cliInstall" = CliInstall: Watch => super::cli_install::status().await?,
    "installCli" = InstallCli: Owner, counted => super::cli_install::install().await?,
    "renameAccountDevice" = RenameAccountDevice: Owner => super::account::devices::rename_device(&core, params).await?,
    "removeAccountDevice" = RemoveAccountDevice: Owner => super::account::devices::remove_device(&core, params).await?,
    "accountPhones" = AccountPhones: Owner => super::account::devices::phones(&core).await?,
    "showFiles" = ShowFiles: Watch => super::open_files::show(&core, client.id, params)?,
    "plugins" = Plugins: Watch => plugins::listed(&core).await,
    "pluginSetDisabled" = PluginSetDisabled: Owner => plugins::set_disabled(&core, params),
    "pluginStream" = PluginStream: Owner => plugins::stream(&core, super::connection::connection_of(&core, client)?, params)?,
    "pluginSignIn": Owner, any_program => {
        plugins::not_an_agent(client).await?;
        answer(plugins::sign_in(&core, super::connection::connection_of(&core, client)?, parse(params)?)?)
    },
    "signInCallback": Owner, any_program => {
        plugins::not_an_agent(client).await?;
        answer(plugins::sign_in_callback(&core, parse(params)?)?)
    },
    "files.list" = FilesList: Full => edits::on_disk(move || crate::files::list(params)).await?,
    "files.kinds" = FilesKinds: Full => edits::on_disk(move || crate::files::kinds(params)).await?,
    "files.read" = FilesRead: Full => edits::on_disk(move || crate::files::read(params, crate::files::PIECE_BYTES)).await?,
    "files.create" = FilesCreate: Full => edits::on_disk(move || crate::files::create(params)).await?,
    "files.rename" = FilesRename: Full => edits::on_disk(move || crate::files::rename(params)).await?,
    "files.delete" = FilesDelete: Full => edits::on_disk(move || crate::files::delete(params)).await?,
    "files.readText" = FilesReadText: Full => edits::on_disk(move || crate::files::read_text(params)).await?,
    "files.copyInto" = FilesCopyInto: Full => edits::on_disk(move || crate::files::copy_into(params)).await?,
    "files.writeBytes" = FilesWriteBytes: Full => edits::on_disk(move || crate::files::write_bytes(params)).await?,
    "files.preview" = FilesPreview: Full => edits::on_disk(move || crate::files::preview(params)).await?,
    "files.bytes" = FilesBytes: Full => edits::on_disk(move || crate::files::bytes(params)).await?,
    "files.write" = FilesWrite: Full => edits::write_file(&core, params).await?,
    "lsp.release" = LspRelease: Full => super::lsp::release(client.id),
    "lsp.claim" = LspClaim: Full => lsp::claim(client.id, params),
    "lsp.languages" = LspLanguages: Full => lsp::languages().await?,
    "lsp.install" = LspInstall: Full => lsp::install(params).await?,
    "lsp.start" = LspStart: Full => lsp::start(&core, params).await?,
    "lsp.stop" = LspStop: Full => lsp::stop(params).await?,
    "lsp.open" = LspOpen: Full => lsp::open(client.id, params).await?,
    "lsp.change" = LspChange: Full => lsp::change(params).await?,
    "lsp.changeIncremental" = LspChangeIncremental: Full => lsp::change_incremental(params).await?,
    "lsp.save" = LspSave: Full => lsp::save(params).await?,
    "lsp.close" = LspClose: Full => lsp::close(client.id, params).await?,
    "lsp.locations" = LspLocations: Full, counted => lsp::locations(params).await?,
    "lsp.symbols" = LspSymbols: Full => lsp::symbols(params).await?,
    "lsp.diagnostics" = LspDiagnostics: Full => lsp::diagnostics_of(params),
    "git.pullRequestUrl" = GitPullRequestUrl: Watch, any_program => super::git::pull_request_url(&core, &client.peer, params).await?,
}

pub(super) fn parse<T: for<'de> Deserialize<'de>>(params: Value) -> CoreResult<T> {
    serde_json::from_value(params)
        .map_err(|error| CoreError::from(format!("the call's params do not fit: {error}")))
}

/// What a row does to its agent, as the calls above run it, for the host an agent's lead runs
/// on: [`ROW_ACTIONS`] names the same methods.
pub(crate) async fn row_action(core: &Arc<Core>, method: &str, params: Value) -> CoreResult<Value> {
    match method {
        "sleepAgent" => {
            let params: AgentParams = parse(params)?;
            answer(super::lifecycle::sleep_agent(core, &params.agent_id)?)
        }
        "renameAgent" => answer(names::rename_agent(core, parse(params)?).await?),
        "updateAgent" => answer(workspace::update_agent(core, parse(params)?)?),
        "closeAgents" => answer(workspace::close_agents(core, parse(params)?)?),
        "snoozeAgents" => answer(super::settling::snooze(core, parse(params)?)?),
        "reopenAgent" => {
            let params: AgentParams = parse(params)?;
            let taken = super::launching::reopen_agent(core, params.agent_id).await?;
            super::launching::wake_chat(core, taken.agent_id.clone()).await?;
            answer(taken)
        }
        "settleAgents" => answer(super::settling::settle(core, parse(params)?)?),
        method => Err(format!("{method} is not something a row does to its agent").into()),
    }
}

/// The calls a row makes about its agent, which the host the agent runs on answers.
pub(crate) const ROW_ACTIONS: [&str; 7] = [
    "sleepAgent",
    "reopenAgent",
    "renameAgent",
    "updateAgent",
    "closeAgents",
    "snoozeAgents",
    "settleAgents",
];

fn answer(result: impl Serialize) -> CoreResult<Value> {
    Ok(serde_json::to_value(result)?)
}

/// Where the person's own agent detection rules live.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestFolder {
    manifest_dir: Option<std::path::PathBuf>,
}
