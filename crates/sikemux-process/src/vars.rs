//! The environment variables Sikemux sets for the programs it starts.

pub const AGENT_ID: &str = "SIKEMUX_AGENT_ID";
pub const AGENT_TYPE: &str = "SIKEMUX_AGENT_TYPE";
pub const TOOLS_AGENT_ID: &str = "SIKEMUX_TOOLS_AGENT_ID";
pub const CLI_ENDPOINT: &str = "SIKEMUX_CLI_ENDPOINT";
pub const CONFIG_DIR: &str = "SIKEMUX_CONFIG_DIR";
/// Set when the app opens, it opens its window without coming in front of the app in use.
pub const LAUNCH_BEHIND: &str = "SIKEMUX_LAUNCH_BEHIND";
pub const CLI_ENDPOINT_PUBLISH: &str = "SIKEMUX_CLI_ENDPOINT_PUBLISH";
pub const REAL_GIT: &str = "SIKEMUX_REAL_GIT";
pub const USER_ZDOTDIR: &str = "SIKEMUX_USER_ZDOTDIR";
pub const PROJECT: &str = "SIKEMUX_PROJECT";
pub const SHELL: &str = "SIKEMUX_SHELL";
pub const TOOLS_MCP_COMMAND: &str = "SIKEMUX_TOOLS_MCP_COMMAND";
pub const TOOLS_MCP_ARGS: &str = "SIKEMUX_TOOLS_MCP_ARGS";
pub const TASK_EXECUTION_ID: &str = "SIKEMUX_TASK_EXECUTION_ID";
pub const TASK_TERMINAL_KEY: &str = "SIKEMUX_TASK_TERMINAL_KEY";
pub const TASK_ID: &str = "SIKEMUX_TASK_ID";
pub const TASK_SOURCE: &str = "SIKEMUX_TASK_SOURCE";
pub const VERSION: &str = "SIKEMUX_VERSION";
pub const SESSION_ID: &str = "SIKEMUX_SESSION_ID";
pub const SESSION_NAME: &str = "SIKEMUX_SESSION_NAME";
pub const SESSION_KIND: &str = "SIKEMUX_SESSION_KIND";
pub const WINDOW_ID: &str = "SIKEMUX_WINDOW_ID";
pub const PANE_ID: &str = "SIKEMUX_PANE_ID";
pub const BIN_PATH: &str = "SIKEMUX_BIN_PATH";
pub const SHELL_INTEGRATION: &str = "SIKEMUX_SHELL_INTEGRATION";
/// `path` in a sandbox's own environment: agent CLIs are looked up only on the PATH it was
/// started with, never in the person's login shell or the folders installers use.
pub const AGENT_SEARCH: &str = "SIKEMUX_AGENT_SEARCH";

/// The Claude home of the agent session that started this process. No agent, terminal or task
/// Sikemux starts inherits it; an account's profile sets its own again.
pub const CLAUDE_HOME: &str = "CLAUDE_CONFIG_DIR";

/// The caller a headless Claude Code names on its requests. Plans count the SDK's own names
/// (`sdk-ts`, `sdk-cli`) against the 5-hour limit about twice as fast: anthropics/claude-code#97074.
pub const CLAUDE_ENTRYPOINT: (&str, &str) = ("CLAUDE_CODE_ENTRYPOINT", "sikemux");

/// What the agent session that started this process set for its own programs, which no agent,
/// terminal or task Sikemux starts may inherit: Claude Code's session markers, which turn a new
/// agent's transcript saving off and hand it the session's messaging socket, its Claude home, and
/// Sikemux's names for that session's agent, which Sikemux sets afresh for each program. Claude
/// Code's own settings, such as `CLAUDE_CODE_USE_BEDROCK`, are the person's and go through.
pub const LAUNCHING_SESSION: [&str; 17] = [
    "CLAUDECODE",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_EFFORT",
    "CLAUDE_PID",
    CLAUDE_HOME,
    AGENT_ID,
    AGENT_TYPE,
    "SIKEMUX_AGENT_HOOKS",
    TOOLS_AGENT_ID,
    CLI_ENDPOINT,
    "SIKEMUX_SKILLS_DIR",
    REAL_GIT,
];
