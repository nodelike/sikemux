use std::collections::HashMap;
use std::path::{Path, PathBuf};

use portable_pty::CommandBuilder;

use crate::error::{PtyError, PtyResult};
use crate::shell::{detect_shell_kind, ShellKind};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PtyContext {
    pub session_id: String,
    pub session_name: String,
    pub session_kind: String,
    pub project: Option<String>,
    pub window_id: Option<String>,
    pub pane_id: Option<String>,
    pub agent_id: Option<String>,
    pub agent_type: Option<String>,
    #[serde(default)]
    pub initial_prompt_submitted: bool,
    /// Explicit opt-in. Absent/false preserves the exact historical shell
    /// launch path and performs no startup-file or argv injection.
    #[serde(default)]
    pub shell_integration: bool,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PtyAgentProfile {
    pub config_path: Option<String>,
    #[serde(default)]
    environment_keys: Vec<String>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PtyDirectCommand {
    pub program: String,
    pub args: Vec<String>,
    pub profile: Option<PtyAgentProfile>,
}

pub fn validate_direct_command(
    command: &PtyDirectCommand,
    context: Option<&PtyContext>,
) -> PtyResult<()> {
    if context
        .and_then(|value| value.agent_id.as_deref())
        .is_none()
        || context
            .and_then(|value| value.agent_type.as_deref())
            .is_none()
    {
        return Err(PtyError::BadArg(
            "direct PTY commands require an explicit agent context",
        ));
    }
    if command.program.is_empty()
        || command.program.len() > 4_096
        || command.program.contains('\0')
        || command.args.len() > 128
    {
        return Err(PtyError::BadArg("invalid direct PTY command"));
    }
    let mut total = command.program.len();
    for argument in &command.args {
        if argument.len() > 8_192 || argument.contains('\0') {
            return Err(PtyError::BadArg("invalid direct PTY command argument"));
        }
        total = total.saturating_add(argument.len());
    }
    if let Some(profile) = command.profile.as_ref() {
        if profile
            .config_path
            .as_deref()
            .is_some_and(|path| path.len() > 4_096 || path.contains('\0'))
            || profile.environment_keys.len() > 64
            || profile.environment_keys.iter().any(|key| {
                key.len() > 128
                    || !key.chars().enumerate().all(|(index, character)| {
                        character == '_'
                            || character.is_ascii_alphanumeric()
                                && (index > 0 || !character.is_ascii_digit())
                    })
            })
        {
            return Err(PtyError::BadArg("invalid direct PTY agent profile"));
        }
    }
    if total > 64 * 1_024 {
        return Err(PtyError::BadArg("direct PTY command is too large"));
    }
    Ok(())
}

pub fn configure_interactive_command(
    command: &mut CommandBuilder,
    shell: &str,
    launch: &PtyDirectCommand,
) {
    let kind = detect_shell_kind(shell);
    let invocation = std::iter::once(&launch.program)
        .chain(launch.args.iter())
        .map(|value| quote_shell_word(value, kind))
        .collect::<Vec<_>>()
        .join(" ");
    #[cfg(unix)]
    command.args(["-l", "-i", "-c", &invocation]);
    #[cfg(windows)]
    command.args(["-NoLogo", "-NoExit", "-Command", &format!("& {invocation}")]);
}

fn quote_shell_word(value: &str, kind: Option<ShellKind>) -> String {
    match kind {
        Some(ShellKind::Fish) => {
            format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
        }
        Some(ShellKind::PowerShell) => format!("'{}'", value.replace('\'', "''")),
        _ if cfg!(windows) => format!("'{}'", value.replace('\'', "''")),
        _ => format!("'{}'", value.replace('\'', "'\\''")),
    }
}

fn agent_profile_config_root(path: &str, home: &Path) -> PathBuf {
    let trimmed = path.trim();
    let expanded = if trimmed == "~" {
        home.to_path_buf()
    } else if let Some(rest) = trimmed
        .strip_prefix("~/")
        .or_else(|| trimmed.strip_prefix("~\\"))
    {
        home.join(rest)
    } else {
        PathBuf::from(trimmed)
    };
    if matches!(
        expanded.file_name().and_then(|value| value.to_str()),
        Some("config.toml" | "settings.json" | "settings.local.json")
    ) {
        expanded.parent().map(Path::to_path_buf).unwrap_or(expanded)
    } else {
        expanded
    }
}

pub fn apply_agent_profile(
    command: &mut CommandBuilder,
    context: Option<&PtyContext>,
    profile: Option<&PtyAgentProfile>,
    home: &Path,
) {
    let Some(root) = profile
        .and_then(|profile| profile.config_path.as_deref())
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(|path| agent_profile_config_root(path, home))
    else {
        return;
    };
    match context.and_then(|context| context.agent_type.as_deref()) {
        Some("codex") => command.env("CODEX_HOME", root),
        Some("claude") => command.env("CLAUDE_CONFIG_DIR", root),
        _ => {}
    }
}

pub const OPTIONAL_PTY_ENV: &[&str] = &[
    "SIKEMUX_SHELL",
    "SIKEMUX_SESSION_ID",
    "SIKEMUX_SESSION_NAME",
    "SIKEMUX_SESSION_KIND",
    "SIKEMUX_PROJECT",
    "SIKEMUX_WINDOW_ID",
    "SIKEMUX_PANE_ID",
    "SIKEMUX_AGENT_ID",
    "SIKEMUX_AGENT_TYPE",
    "SIKEMUX_BIN_PATH",
    "SIKEMUX_CLI_ENDPOINT",
    "SIKEMUX_CLI_ENDPOINT_PUBLISH",
    "SIKEMUX_SHELL_INTEGRATION",
    "SIKEMUX_ORIGINAL_ZDOTDIR",
    "SIKEMUX_ORIGINAL_ZDOTDIR_SET",
    "SIKEMUX_TEMP_ZDOTDIR",
    "SIKEMUX_ORIGINAL_XDG_CONFIG_HOME",
    "SIKEMUX_ORIGINAL_FISH_CONFIG",
    "SIKEMUX_TASK_EXECUTION_ID",
    "SIKEMUX_TASK_TERMINAL_KEY",
    "SIKEMUX_TASK_ID",
    "SIKEMUX_TASK_SOURCE",
    "SIKEMUX_BROWSER_STATE_DIR",
    "SIKEMUX_BROWSER_CDP_URL",
    "SIKEMUX_BROWSER_BROKER_URL",
    "SIKEMUX_BROWSER_BROKER_TOKEN",
    "SIKEMUX_TOOLS_MCP_COMMAND",
    "SIKEMUX_TOOLS_MCP_ARGS",
    "SIKEMUX_TOOLS_AGENT_ID",
];

fn non_empty(value: &Option<String>) -> Option<&str> {
    value.as_deref().filter(|value| !value.is_empty())
}

fn editor_command(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let shell_safe = raw.bytes().all(|byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'\\' | b':' | b'.' | b'_' | b'-')
    });
    if shell_safe {
        return raw.into_owned();
    }
    #[cfg(windows)]
    {
        format!("\"{}\"", raw.replace('"', "\\\""))
    }
    #[cfg(not(windows))]
    {
        format!("'{}'", raw.replace('\'', "'\\''"))
    }
}

/// Apply a clean, typed Sikemux identity to a PTY command. Optional fields are
/// removed before being rebuilt so a terminal can never inherit the identity
/// of the app's parent terminal (or a Codex thread that launched the app).
pub fn configure_pty_environment(
    cmd: &mut CommandBuilder,
    context: Option<&PtyContext>,
    version: &str,
    cli_executable: Option<&Path>,
    cli_endpoint: Option<&Path>,
    profile_env: &HashMap<String, String>,
) {
    for (key, value) in profile_env {
        if cmd.get_env(key).is_none() {
            cmd.env(key, value);
        }
    }

    for key in OPTIONAL_PTY_ENV {
        cmd.env_remove(key);
    }

    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    cmd.env("TERM_PROGRAM", "Sikemux");
    cmd.env("TERM_PROGRAM_VERSION", version);
    cmd.env("SIKEMUX", "1");
    cmd.env("SIKEMUX_VERSION", version);

    if let Some(context) = context {
        cmd.env("SIKEMUX_SESSION_ID", &context.session_id);
        cmd.env("SIKEMUX_SESSION_NAME", &context.session_name);
        cmd.env("SIKEMUX_SESSION_KIND", &context.session_kind);
        if let Some(project) = non_empty(&context.project) {
            cmd.env("SIKEMUX_PROJECT", project);
        }
        if let Some(window_id) = non_empty(&context.window_id) {
            cmd.env("SIKEMUX_WINDOW_ID", window_id);
        }
        if let Some(pane_id) = non_empty(&context.pane_id) {
            cmd.env("SIKEMUX_PANE_ID", pane_id);
        }
        if let Some(agent_id) = non_empty(&context.agent_id) {
            cmd.env("SIKEMUX_AGENT_ID", agent_id);
        }
        if let Some(agent_type) = non_empty(&context.agent_type) {
            cmd.env("SIKEMUX_AGENT_TYPE", agent_type);
        }
    }

    if let Some(path) = cli_executable {
        cmd.env("SIKEMUX_BIN_PATH", path);
        if cmd.get_env("EDITOR").is_none() && cmd.get_env("VISUAL").is_none() {
            let editor = editor_command(path);
            cmd.env("EDITOR", &editor);
            cmd.env("VISUAL", &editor);
        }
    }
    if let Some(path) = cli_endpoint {
        cmd.env("SIKEMUX_CLI_ENDPOINT", path);
    }
}

#[cfg(test)]
mod tests {
    use super::{apply_agent_profile, configure_pty_environment, PtyAgentProfile, PtyContext};
    use crate::tests::{env, local_shell_context};
    use portable_pty::CommandBuilder;
    use std::collections::HashMap;
    use std::path::Path;

    #[cfg(target_os = "macos")]
    #[test]
    fn interactive_command_reads_zshrc_and_preserves_literal_arguments() {
        use portable_pty::{NativePtySystem, PtySize, PtySystem};
        use std::io::Read;

        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join(".zshrc"),
            "export SIKEMUX_TEST_AUTH=from-zshrc\nprobe() { printf '%s|%s' \"$SIKEMUX_TEST_AUTH\" \"$1\"; }\n").unwrap();
        let mut command = CommandBuilder::new("/bin/zsh");
        command.env("ZDOTDIR", root.path());
        command.env_remove("SIKEMUX_TEST_AUTH");
        let argument = "spaces ' quotes; $(printf injected) \n next";
        super::configure_interactive_command(
            &mut command,
            "/bin/zsh",
            &super::PtyDirectCommand {
                program: "probe".into(),
                args: vec![argument.into()],
                profile: None,
            },
        );
        let pair = NativePtySystem::default()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let mut reader = pair.master.try_clone_reader().unwrap();
        let mut child = pair.slave.spawn_command(command).unwrap();
        drop(pair.slave);
        let mut output = Vec::new();
        let _ = reader.read_to_end(&mut output);
        assert_eq!(child.wait().unwrap().exit_code(), 0);
        assert!(String::from_utf8_lossy(&output)
            .replace("\r\n", "\n")
            .contains(&format!("from-zshrc|{argument}")));
    }

    #[test]
    fn fish_words_escape_backslashes_and_quotes_inside_single_quotes() {
        let fish = Some(super::ShellKind::Fish);
        assert_eq!(super::quote_shell_word("plain", fish), "'plain'");
        assert_eq!(super::quote_shell_word("it's", fish), r"'it\'s'");
        assert_eq!(super::quote_shell_word(r"ends\", fish), r"'ends\\'");
        assert_eq!(
            super::quote_shell_word(r"a\'; rm -rf ~; echo '", fish),
            r"'a\\\'; rm -rf ~; echo \''"
        );
        #[cfg(unix)]
        assert_eq!(
            super::quote_shell_word(r"it's\", Some(super::ShellKind::Zsh)),
            r"'it'\''s\'"
        );
    }

    #[test]
    fn agent_profiles_pin_provider_config_directories() {
        let profile = PtyAgentProfile {
            config_path: Some("/profiles/work/config.toml".into()),
            environment_keys: Vec::new(),
        };
        let mut codex = CommandBuilder::new("codex");
        let mut context = local_shell_context();
        context.agent_id = Some("agent-1".into());
        context.agent_type = Some("codex".into());
        apply_agent_profile(
            &mut codex,
            Some(&context),
            Some(&profile),
            Path::new("/home"),
        );
        assert_eq!(env(&codex, "CODEX_HOME"), Some("/profiles/work".into()));

        let mut claude = CommandBuilder::new("claude");
        context.agent_type = Some("claude".into());
        apply_agent_profile(
            &mut claude,
            Some(&context),
            Some(&profile),
            Path::new("/home"),
        );
        assert_eq!(
            env(&claude, "CLAUDE_CONFIG_DIR"),
            Some("/profiles/work".into())
        );
    }

    #[test]
    fn pty_environment_has_terminal_and_typed_sikemux_identity() {
        let mut command = CommandBuilder::new("shell");
        command.env("SIKEMUX_AGENT_ID", "stale-agent");
        command.env("CODEX_THREAD_ID", "parent-thread");
        command.env("CLAUDECODE", "1");
        command.env("CLAUDE_CODE_CHILD_SESSION", "1");
        command.env("CLAUDE_CODE_SESSION_ID", "parent-session");
        command.env("CLAUDE_CODE_MESSAGING_TOKEN", "parent-token");
        command.env("CLAUDE_CODE_MESSAGING_SOCKET", "/tmp/parent.sock");
        command.env_remove("EDITOR");
        command.env_remove("VISUAL");
        let context = PtyContext {
            session_id: "session-1".into(),
            session_name: "repo".into(),
            session_kind: "project".into(),
            project: Some("/repo".into()),
            window_id: Some("window-1".into()),
            pane_id: Some("pane-1".into()),
            agent_id: None,
            agent_type: None,
            initial_prompt_submitted: false,
            shell_integration: false,
        };

        configure_pty_environment(
            &mut command,
            Some(&context),
            "1.2.3",
            Some(Path::new("/app/sikemux-editor")),
            Some(Path::new("/runtime/cli.json")),
            &HashMap::new(),
        );

        assert_eq!(env(&command, "TERM"), Some("xterm-256color".into()));
        assert_eq!(env(&command, "COLORTERM"), Some("truecolor".into()));
        assert_eq!(env(&command, "TERM_PROGRAM"), Some("Sikemux".into()));
        assert_eq!(env(&command, "TERM_PROGRAM_VERSION"), Some("1.2.3".into()));
        assert_eq!(env(&command, "SIKEMUX"), Some("1".into()));
        assert_eq!(env(&command, "SIKEMUX_VERSION"), Some("1.2.3".into()));
        assert_eq!(
            env(&command, "SIKEMUX_SESSION_ID"),
            Some("session-1".into())
        );
        assert_eq!(env(&command, "SIKEMUX_SESSION_NAME"), Some("repo".into()));
        assert_eq!(
            env(&command, "SIKEMUX_SESSION_KIND"),
            Some("project".into())
        );
        assert_eq!(env(&command, "SIKEMUX_PROJECT"), Some("/repo".into()));
        assert_eq!(env(&command, "SIKEMUX_WINDOW_ID"), Some("window-1".into()));
        assert_eq!(env(&command, "SIKEMUX_PANE_ID"), Some("pane-1".into()));
        assert_eq!(env(&command, "SIKEMUX_AGENT_ID"), None);
        assert_eq!(
            env(&command, "CODEX_THREAD_ID"),
            Some("parent-thread".into())
        );
        for marker in [
            "CLAUDECODE",
            "CLAUDE_CODE_CHILD_SESSION",
            "CLAUDE_CODE_SESSION_ID",
            "CLAUDE_CODE_MESSAGING_TOKEN",
            "CLAUDE_CODE_MESSAGING_SOCKET",
        ] {
            assert!(env(&command, marker).is_some());
        }
        assert_eq!(
            env(&command, "SIKEMUX_BIN_PATH"),
            Some("/app/sikemux-editor".into())
        );
        assert_eq!(
            env(&command, "SIKEMUX_CLI_ENDPOINT"),
            Some("/runtime/cli.json".into())
        );
        assert_eq!(env(&command, "EDITOR"), Some("/app/sikemux-editor".into()));
        assert_eq!(env(&command, "VISUAL"), Some("/app/sikemux-editor".into()));
    }

    #[test]
    fn pty_environment_preserves_user_editor_choice_if_either_var_exists() {
        let mut editor_only = CommandBuilder::new("shell");
        editor_only.env("EDITOR", "nvim");
        editor_only.env_remove("VISUAL");
        configure_pty_environment(
            &mut editor_only,
            None,
            "1.2.3",
            Some(Path::new("/app/sikemux-editor")),
            None,
            &HashMap::new(),
        );
        assert_eq!(env(&editor_only, "EDITOR"), Some("nvim".into()));
        assert_eq!(env(&editor_only, "VISUAL"), None);

        let mut visual_only = CommandBuilder::new("shell");
        visual_only.env_remove("EDITOR");
        visual_only.env("VISUAL", "code --wait");
        configure_pty_environment(
            &mut visual_only,
            None,
            "1.2.3",
            Some(Path::new("/app/sikemux-editor")),
            None,
            &HashMap::new(),
        );
        assert_eq!(env(&visual_only, "EDITOR"), None);
        assert_eq!(env(&visual_only, "VISUAL"), Some("code --wait".into()));
    }

    #[test]
    fn pty_environment_quotes_editor_command_paths_with_spaces() {
        let mut command = CommandBuilder::new("shell");
        command.env_remove("EDITOR");
        command.env_remove("VISUAL");
        configure_pty_environment(
            &mut command,
            None,
            "1.2.3",
            Some(Path::new(
                "/Applications/Sikemux Preview.app/Contents/MacOS/sikemux-editor",
            )),
            None,
            &HashMap::new(),
        );

        assert_eq!(
            env(&command, "SIKEMUX_BIN_PATH"),
            Some("/Applications/Sikemux Preview.app/Contents/MacOS/sikemux-editor".into())
        );
        #[cfg(not(windows))]
        assert_eq!(
            env(&command, "EDITOR"),
            Some("'/Applications/Sikemux Preview.app/Contents/MacOS/sikemux-editor'".into())
        );
        #[cfg(windows)]
        assert_eq!(
            env(&command, "EDITOR"),
            Some("\"/Applications/Sikemux Preview.app/Contents/MacOS/sikemux-editor\"".into())
        );
    }

    /// The bug this guards: an agent CLI spawned as a direct command reads no
    /// shell profile, so a credential the user keeps in `.zshrc` never reaches
    /// it and the CLI demands a login that the user's own terminal never asks
    /// for.
    #[test]
    fn pty_environment_supplies_profile_exports_a_shell_free_pane_cannot_read() {
        let mut command = CommandBuilder::new("claude");
        command.env_remove("ANTHROPIC_API_KEY");
        let profile = HashMap::from([
            ("ANTHROPIC_API_KEY".to_string(), "sk-profile".to_string()),
            ("HTTPS_PROXY".to_string(), "http://proxy:8080".to_string()),
        ]);

        configure_pty_environment(&mut command, None, "1.2.3", None, None, &profile);

        assert_eq!(
            env(&command, "ANTHROPIC_API_KEY"),
            Some("sk-profile".into())
        );
        assert_eq!(
            env(&command, "HTTPS_PROXY"),
            Some("http://proxy:8080".into())
        );
    }

    /// The profile fills gaps; it never overrules a value this layer sets. A
    /// profile that exports TERM_PROGRAM must not make a pane claim to be
    /// another terminal, and one that exports SIKEMUX_AGENT_ID must not let a
    /// pane forge another pane's identity.
    #[test]
    fn pty_environment_profile_never_overrides_sikemux_owned_identity() {
        let mut command = CommandBuilder::new("shell");
        let context = PtyContext {
            session_id: "session-1".into(),
            session_name: "one".into(),
            session_kind: "agent".into(),
            project: None,
            window_id: None,
            pane_id: None,
            agent_id: Some("agent-real".into()),
            agent_type: Some("claude".into()),
            initial_prompt_submitted: false,
            shell_integration: false,
        };
        let profile = HashMap::from([
            ("TERM_PROGRAM".to_string(), "Ghostty".to_string()),
            ("TERM".to_string(), "xterm-kitty".to_string()),
            ("SIKEMUX_AGENT_ID".to_string(), "agent-forged".to_string()),
        ]);

        configure_pty_environment(&mut command, Some(&context), "1.2.3", None, None, &profile);

        assert_eq!(env(&command, "TERM_PROGRAM"), Some("Sikemux".into()));
        assert_eq!(env(&command, "TERM"), Some("xterm-256color".into()));
        assert_eq!(env(&command, "SIKEMUX_AGENT_ID"), Some("agent-real".into()));
    }

    #[test]
    fn pty_environment_preserves_provider_environment_and_resets_own_identity() {
        let mut command = CommandBuilder::new("claude");
        // `CommandBuilder::new` seeds itself from this process's environment,
        // so a developer running the suite from their own terminal already has
        // these set and the profile fill would correctly decline to touch them.
        // Clear them to model the launchd-minimal environment a GUI app sees.
        command.env_remove("ANTHROPIC_API_KEY");
        let context = PtyContext {
            session_id: "session-1".into(),
            session_name: "one".into(),
            session_kind: "agent".into(),
            project: None,
            window_id: None,
            pane_id: None,
            agent_id: None,
            agent_type: None,
            initial_prompt_submitted: false,
            shell_integration: false,
        };
        let profile = HashMap::from([
            ("SIKEMUX_AGENT_ID".to_string(), "agent-forged".to_string()),
            ("SIKEMUX_PANE_ID".to_string(), "pane-forged".to_string()),
            ("SIKEMUX_SHELL_INTEGRATION".to_string(), "1".to_string()),
            ("CLAUDECODE".to_string(), "1".to_string()),
            (
                "CLAUDE_CODE_MESSAGING_SOCKET".to_string(),
                "/tmp/parent.sock".to_string(),
            ),
            (
                "CLAUDE_CODE_MESSAGING_TOKEN".to_string(),
                "parent-token".to_string(),
            ),
            ("CODEX_THREAD_ID".to_string(), "thread-parent".to_string()),
            // A real credential in the same map still has to arrive, or the
            // scrub would be passing by discarding everything.
            ("ANTHROPIC_API_KEY".to_string(), "sk-profile".to_string()),
        ]);

        configure_pty_environment(&mut command, Some(&context), "1.2.3", None, None, &profile);

        for scrubbed in [
            "SIKEMUX_AGENT_ID",
            "SIKEMUX_PANE_ID",
            "SIKEMUX_SHELL_INTEGRATION",
        ] {
            assert_eq!(env(&command, scrubbed), None);
        }
        for key in [
            "CLAUDECODE",
            "CLAUDE_CODE_MESSAGING_SOCKET",
            "CLAUDE_CODE_MESSAGING_TOKEN",
            "CODEX_THREAD_ID",
        ] {
            assert!(env(&command, key).is_some());
        }
        assert_eq!(
            env(&command, "ANTHROPIC_API_KEY"),
            Some("sk-profile".into())
        );
    }

    /// An explicit value already on the command outranks the profile, so a
    /// per-pane assignment is never silently replaced by a global export.
    #[test]
    fn pty_environment_profile_yields_to_an_explicit_command_value() {
        let mut command = CommandBuilder::new("shell");
        command.env("ANTHROPIC_API_KEY", "sk-explicit");
        let profile = HashMap::from([("ANTHROPIC_API_KEY".to_string(), "sk-profile".to_string())]);

        configure_pty_environment(&mut command, None, "1.2.3", None, None, &profile);

        assert_eq!(
            env(&command, "ANTHROPIC_API_KEY"),
            Some("sk-explicit".into())
        );
    }

    #[test]
    fn pty_environment_rebuilds_agent_identity_without_fake_pane_identity() {
        let mut command = CommandBuilder::new("shell");
        command.env("SIKEMUX_WINDOW_ID", "parent-window");
        command.env("SIKEMUX_PANE_ID", "parent-pane");
        let context = PtyContext {
            session_id: "session-1".into(),
            session_name: "repo".into(),
            session_kind: "project".into(),
            project: Some("/repo".into()),
            window_id: None,
            pane_id: None,
            agent_id: Some("agent-1".into()),
            agent_type: Some("codex".into()),
            initial_prompt_submitted: false,
            shell_integration: false,
        };

        configure_pty_environment(
            &mut command,
            Some(&context),
            "1.2.3",
            None,
            None,
            &HashMap::new(),
        );

        assert_eq!(env(&command, "SIKEMUX_AGENT_ID"), Some("agent-1".into()));
        assert_eq!(env(&command, "SIKEMUX_AGENT_TYPE"), Some("codex".into()));
        assert_eq!(env(&command, "SIKEMUX_WINDOW_ID"), None);
        assert_eq!(env(&command, "SIKEMUX_PANE_ID"), None);
    }
}
