use super::*;

const PROJECT_PATH: &str = if cfg!(windows) {
    r"C:\Users\me\sikemux"
} else {
    "/Users/me/sikemux"
};

fn launcher() -> Launcher {
    Launcher {
        id: "claude:work".into(),
        label: "Claude Code (work)".into(),
        agent: AgentSpec {
            config_path: Some("/Users/me/.claude-work".into()),
            ..AgentSpec::new("claude")
        },
        permission_mode: PermissionMode::Bypass,
        status: None,
        program: Some("/usr/local/bin/claude".into()),
    }
}

fn offer(workspaces: &Workspaces, launchers: Vec<Launcher>) {
    let project = project();
    workspaces
        .add_project(project.path, Some(project.name), Some(project.id))
        .unwrap();
    workspaces.set_launchers(launchers);
}

fn launch(workspaces: &Workspaces, choice: ChatChoice) -> CoreResult<ChatLaunch> {
    let launcher = workspaces.launcher(&choice.launcher)?;
    let cwd = workspaces.project_path(&choice.project)?;
    let program = launcher.program.clone().unwrap_or_default();
    let mut launch = workspaces.chat_launch(&launcher, program, &cwd, None, None)?;
    launch.resume_id = choice.resume_id;
    launch.model = choice.model;
    Ok(launch)
}

fn project() -> ProjectInfo {
    ProjectInfo {
        id: "sess-1".into(),
        name: "sikemux".into(),
        path: PROJECT_PATH.into(),
        pinned: false,
        added_at: 0,
        space: None,
        space_parent: None,
        space_followed: false,
        code_hosts: Default::default(),
        identity: None,
        copies: Vec::new(),
        git: None,
        launcher: None,
        code_host: None,
        worktree_default: false,
        saved_chats: BTreeMap::new(),
        chief: None,
        chief_count: 0,
        chief_home: None,
        agents: 0,
        standing: Default::default(),
        calls: 0,
        cloning: None,
        shared: None,
        browser_profile: None,
    }
}

fn choice() -> ChatChoice {
    ChatChoice {
        launcher: "claude:work".into(),
        project: "sess-1".into(),
        permission_mode: None,
        model: Some("opus".into()),
        effort: None,
        resume_id: None,
        cwd: None,
        title: None,
        terminal: false,
    }
}

fn recent(session_id: &str, cwd: &str) -> Recent {
    Recent {
        launcher: "claude:work".into(),
        provider: "claude".into(),
        session_id: session_id.into(),
        title: "Fix the login flake".into(),
        project: cwd.into(),
        cwd: cwd.into(),
        active_at: 1_000,
    }
}

fn resuming(recent: &str) -> ResumeChoice {
    ResumeChoice {
        recent: recent.into(),
        permission_mode: None,
        model: None,
        effort: None,
    }
}

#[test]
fn a_device_sees_launchers_without_their_program_or_environment() {
    let workspaces = Workspaces::new(None, None);
    offer(&workspaces, vec![launcher()]);
    let shown = serde_json::to_string(&workspaces.view()).unwrap();
    assert!(shown.contains("Claude Code (work)"));
    assert!(!shown.contains("/usr/local/bin/claude"));
    assert!(!shown.contains(".claude-work"));
}

#[test]
fn a_device_sees_whether_each_launcher_is_signed_in() {
    let workspaces = Workspaces::new(None, None);
    let signed_out = Launcher {
        status: Some(AgentStatus::SignedOut),
        ..launcher()
    };
    offer(&workspaces, vec![signed_out]);
    let view = workspaces.view();
    assert_eq!(view.launchers[0].status.as_deref(), Some("signedOut"));
    offer(&workspaces, vec![launcher()]);
    let shown = serde_json::to_value(workspaces.view()).unwrap();
    assert!(shown["launchers"][0].get("status").is_none());
}

#[test]
fn a_chat_starts_in_the_project_with_the_launcher_and_its_account() {
    let workspaces = Workspaces::new(None, None);
    offer(&workspaces, vec![launcher()]);
    let launch = launch(&workspaces, choice()).unwrap();
    assert_eq!(launch.cwd, PathBuf::from(PROJECT_PATH));
    assert_eq!(launch.program, PathBuf::from("/usr/local/bin/claude"));
    assert_eq!(launch.permission_mode, PermissionMode::Bypass);
    assert_eq!(launch.model.as_deref(), Some("opus"));
    assert_eq!(
        launch.env.get("CLAUDE_CONFIG_DIR").map(String::as_str),
        Some("/Users/me/.claude-work")
    );
}

#[test]
fn a_worker_cut_from_another_repo_stays_in_its_leads_project() {
    let workspaces = Workspaces::new(None, None);
    offer(&workspaces, vec![launcher()]);
    let home = Path::new(PROJECT_PATH).parent().unwrap().to_owned();
    let home = workspaces.add_project(home, None, None).unwrap();
    let mut lead = launch(&workspaces, choice()).unwrap();
    lead.agent_id = "agent-lead".into();
    workspaces.note_begun(&lead, None, None);
    let mut worker = lead.clone();
    worker.agent_id = "agent-worker".into();
    worker.cwd = home.path.join("site.worktrees").join("fix-hero");
    workspaces.note_begun(&worker, Some("agent-lead"), None);
    assert_eq!(workspaces.agent("agent-worker").unwrap().project, "sess-1");
}

#[test]
fn an_unknown_launcher_or_project_is_refused() {
    let workspaces = Workspaces::new(None, None);
    offer(&workspaces, vec![launcher()]);
    let mut wrong = choice();
    wrong.project = "elsewhere".into();
    assert!(launch(&workspaces, wrong).is_err());
    let mut wrong = choice();
    wrong.launcher = "gemini".into();
    assert!(launch(&workspaces, wrong).is_err());
}

#[test]
fn a_device_sees_recent_chats_in_open_projects_with_the_agent_holding_each() {
    let workspaces = Workspaces::new(None, None);
    offer(&workspaces, vec![launcher()]);
    workspaces.set_recent(
        vec![
            recent("s1", PROJECT_PATH),
            recent("s2", PROJECT_PATH),
            recent("s3", "/Users/me/closed"),
        ],
        HashMap::new(),
        HashMap::new(),
    );
    let running = workspaces.listed(Vec::new());
    let shown = workspaces.recent(&running, &Vec::new());
    assert_eq!(
        shown
            .iter()
            .map(|chat| chat.id.as_str())
            .collect::<Vec<_>>(),
        ["claude:s1", "claude:s2"]
    );
    assert_eq!(shown[0].project, "sess-1");
    assert!(!serde_json::to_string(&shown)
        .unwrap()
        .contains("claude:work"));

    let mut live = ChatInfo {
        agent_id: "agent-1".into(),
        name: None,
        provider: "claude".into(),
        title: None,
        cwd: PROJECT_PATH.into(),
        session_id: Some("s1".into()),
        state: ChatState::Ready,
        running: false,
        pending_permissions: Vec::new(),
        subagents: 0,
        tasks: 0,
        reconnecting: None,
        started_by: None,
        launcher: None,
        permission_mode: PermissionMode::WorkspaceWrite,
        model: None,
        model_provider: None,
        effort: None,
        unread: false,
        active_at: None,
        group: Default::default(),
        status: Default::default(),
        next_trigger: None,
        called_at: None,
        attention_rank: 0,
    };
    let held = workspaces.recent(std::slice::from_ref(&live), &Vec::new());
    assert_eq!(held.len(), 2);
    assert_eq!(held[0].agent_id.as_deref(), Some("agent-1"));
    assert_eq!(held[1].agent_id, None);
    live.provider = "codex".into();
    assert_eq!(workspaces.recent(&[live], &Vec::new())[0].agent_id, None);
}

#[test]
fn a_recent_chat_resumes_with_its_launcher_in_its_project() {
    let workspaces = Workspaces::new(None, None);
    offer(&workspaces, vec![launcher()]);
    workspaces.set_recent(
        vec![recent("s1", PROJECT_PATH)],
        HashMap::new(),
        HashMap::new(),
    );
    let choice = workspaces
        .resume_choice(resuming("claude:s1"), &[], &Vec::new())
        .unwrap();
    let launch = launch(&workspaces, choice).unwrap();
    assert_eq!(launch.resume_id.as_deref(), Some("s1"));
    assert_eq!(launch.cwd, PathBuf::from(PROJECT_PATH));
    assert_eq!(launch.provider, "claude");
    assert_eq!(launch.permission_mode, PermissionMode::Bypass);
}

#[test]
fn only_a_published_recent_chat_resumes() {
    let workspaces = Workspaces::new(None, None);
    offer(&workspaces, vec![launcher()]);
    workspaces.set_recent(
        vec![recent("s1", PROJECT_PATH)],
        HashMap::new(),
        HashMap::new(),
    );
    assert!(workspaces
        .resume_choice(resuming("claude:other"), &[], &Vec::new())
        .is_err());
    assert!(workspaces
        .resume_choice(resuming("s1"), &[], &Vec::new())
        .is_err());
    workspaces.set_recent(Vec::new(), HashMap::new(), HashMap::new());
    assert!(workspaces
        .resume_choice(resuming("claude:s1"), &[], &Vec::new())
        .is_err());
    let mut elsewhere = recent("s2", PROJECT_PATH);
    elsewhere.launcher = "codex".into();
    workspaces.set_recent(vec![elsewhere], HashMap::new(), HashMap::new());
    assert!(workspaces
        .resume_choice(resuming("claude:s2"), &[], &Vec::new())
        .is_err());
}

#[test]
fn a_relative_project_path_is_refused() {
    let workspaces = Workspaces::new(None, None);
    assert!(workspaces
        .add_project("sikemux".into(), None, None)
        .is_err());
}

#[test]
fn a_first_prompt_names_an_agent_until_its_chat_titles_it_and_never_one_the_person_named() {
    let workspaces = Workspaces::new(None, None);
    let project = project();
    workspaces
        .add_project(
            project.path.clone(),
            Some(project.name),
            Some(project.id.clone()),
        )
        .unwrap();
    let agent = |id: &str| {
        let agent = serde_json::json!({
            "id": id, "provider": "claude", "project": project.id, "cwd": project.path,
            "title": "claude", "permissionMode": "bypass",
        });
        workspaces
            .put_agent_draft(agent.as_object().unwrap().clone())
            .unwrap();
    };
    let title = |id: &str| workspaces.agent(id).unwrap().title;
    agent("agent-1");
    assert!(!workspaces.note_title("agent-1", "/compact", true));
    let long = format!(
        "Need the binary size on the landing page {}",
        "x".repeat(80)
    );
    assert!(workspaces.note_title("agent-1", &long, true));
    assert_eq!(title("agent-1").chars().count(), STAND_IN_TITLE_CHARS);
    assert!(!workspaces.note_title("agent-1", "A second prompt", true));
    assert!(workspaces.note_title("agent-1", "Binary size on landing page", false));
    assert_eq!(title("agent-1"), "Binary size on landing page");

    agent("agent-2");
    workspaces
        .edit_agent("agent-2", |agent| {
            agent.title = "Parser rewrite".into();
            agent.renamed = true;
        })
        .unwrap();
    assert!(!workspaces.note_title("agent-2", "Fix the parser", true));
    assert!(!workspaces.note_title("agent-2", "Parser fixes", false));
    assert_eq!(title("agent-2"), "Parser rewrite");
}

// A person on the built-in Claude account only, on 0.5.4-nightly.1 and .2:
// their chats ask for plain "claude", the book keeps builtin-claude as a
// profile, so only "claude:builtin-claude" was offered and no chat started.
#[test]
fn a_chat_with_no_profile_starts_on_the_account_the_person_chose() {
    let builtin = crate::agent_accounts::Profile {
        id: "builtin-claude".into(),
        name: "Claude".into(),
        provider: "claude".into(),
        accent: "#fff".into(),
        executable_path: None,
        config_path: None,
        environment_keys: Vec::new(),
    };
    let workspaces = Workspaces::new(None, None);
    workspaces
        .set_accounts(Book {
            profiles: vec![builtin],
            ..Book::default()
        })
        .unwrap();
    assert_eq!(
        workspaces.launcher("claude").unwrap().id,
        "claude:builtin-claude"
    );
    offer(
        &workspaces,
        vec![Launcher {
            id: "claude:builtin-claude".into(),
            ..launcher()
        }],
    );
    assert_eq!(
        workspaces.launcher("claude").unwrap().id,
        "claude:builtin-claude"
    );
}

#[test]
fn each_chat_agent_is_offered_once_or_once_per_account_built_in_included_in_a_mode_it_supports() {
    let profile = |id: &str, name: &str, provider: &str| crate::agent_accounts::Profile {
        id: id.into(),
        name: name.into(),
        provider: provider.into(),
        accent: "#fff".into(),
        executable_path: None,
        config_path: Some(format!("~/.{id}")),
        environment_keys: Vec::new(),
    };
    let workspaces = Workspaces::new(None, None);
    let profiles = vec![
        profile("work", "Work", "claude"),
        profile("home", "Home", "claude"),
        profile("cx", "Default", "codex"),
    ];
    workspaces
        .set_accounts(Book {
            profiles,
            ..Book::default()
        })
        .unwrap();
    workspaces
        .set_permission_mode(PermissionMode::Bypass)
        .unwrap();
    let offered = workspaces.offered();
    let ids: Vec<&str> = offered
        .iter()
        .filter(|launcher| {
            (crate::agent_catalog::AGENTS.iter()).any(|def| def.kind == launcher.agent.provider)
        })
        .map(|launcher| launcher.id.as_str())
        .collect();
    assert_eq!(
        ids,
        [
            "claude:builtin-claude",
            "claude:work",
            "claude:home",
            "codex:builtin-codex",
            "codex:cx",
            "hermes",
            "pi",
            "opencode",
            "omp",
            "grok",
            "antigravity",
            "muse"
        ]
    );
    let by_id = |id: &str| offered.iter().find(|launcher| launcher.id == id).unwrap();
    assert_eq!(by_id("claude:work").label, "Claude · Work");
    assert_eq!(
        by_id("claude:work").agent.config_path.as_deref(),
        Some("~/.work")
    );
    assert_eq!(by_id("codex:cx").label, "Codex · Default");
    assert_eq!(by_id("claude:builtin-claude").agent.config_path, None);
    assert_eq!(by_id("opencode").permission_mode, PermissionMode::Bypass);
    assert_eq!(by_id("grok").permission_mode, PermissionMode::Bypass);
    workspaces
        .set_permission_mode(PermissionMode::Auto)
        .unwrap();
    let offered = workspaces.offered();
    let by_id = |id: &str| offered.iter().find(|launcher| launcher.id == id).unwrap();
    assert_eq!(by_id("claude:work").permission_mode, PermissionMode::Auto);
    assert_eq!(
        by_id("opencode").permission_mode,
        PermissionMode::WorkspaceWrite
    );
}

#[test]
fn a_routed_model_alone_starts_omp_on_its_provider() {
    let data = tempfile::tempdir().unwrap();
    let workspaces = Workspaces::new(Some(data.path()), None);
    workspaces
        .providers()
        .unwrap()
        .add_custom(
            crate::launch::providers::Custom {
                id: "wokey".into(),
                label: "Wokey".into(),
                base_url: "http://127.0.0.1:9/v1".into(),
                keyed: false,
                responses: false,
                anthropic: None,
            },
            None,
        )
        .unwrap();
    let launcher = Launcher {
        id: "omp".into(),
        label: "OMP".into(),
        agent: AgentSpec::new("omp"),
        permission_mode: PermissionMode::Bypass,
        status: None,
        program: Some("/usr/local/bin/omp".into()),
    };
    let launch = workspaces
        .chat_launch(
            &launcher,
            "/usr/local/bin/omp".into(),
            Path::new(PROJECT_PATH),
            None,
            Some("sikemux-wokey/gpt-6.2-sol".into()),
        )
        .unwrap();
    assert_eq!(launch.model_provider.as_deref(), Some("wokey"));
    assert!(launch
        .args
        .windows(2)
        .any(|pair| pair == ["--model", "sikemux-wokey/gpt-6.2-sol"]));
}
