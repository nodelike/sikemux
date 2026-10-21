//! The command line an agent runs with in a terminal: its model, effort,
//! permission mode and the session it resumes, in each CLI's own flags.

use crate::agent_catalog::{self, AgentEntry, Resume};
use crate::launch::PermissionMode;

/// How the person runs a terminal agent.
#[derive(Clone, Debug)]
pub struct TerminalRun<'a> {
    pub resume_id: Option<&'a str>,
    pub permission_mode: PermissionMode,
    pub model: Option<&'a str>,
    pub effort: Option<&'a str>,
}

/// The closest effort the CLI takes to the one the person chose.
fn effort(agent: &AgentEntry, chosen: &str) -> Option<&'static str> {
    let near = if chosen == "ultra" { "max" } else { chosen };
    let supported = agent.efforts.iter().copied();
    supported
        .clone()
        .find(|effort| *effort == chosen)
        .or_else(|| supported.into_iter().find(|effort| *effort == near))
}

/// The CLI's flags for Sikemux's Normal and YOLO modes. YOLO on an agent
/// that has none runs as Normal.
pub fn permission_args(agent: &AgentEntry, mode: PermissionMode) -> Vec<String> {
    let flags = &agent.terminal;
    let args = flags
        .bypass
        .filter(|_| mode == PermissionMode::Bypass)
        .unwrap_or(flags.normal);
    args.iter().map(|arg| (*arg).to_owned()).collect()
}

/// The arguments `provider`'s CLI starts with in a terminal for `run`.
pub fn args(provider: &str, run: &TerminalRun<'_>) -> Vec<String> {
    let Some(agent) = agent_catalog::agent(provider) else {
        return Vec::new();
    };
    let flags = &agent.terminal;
    let model = run.model.map(str::trim).filter(|model| !model.is_empty());
    let effort = run.effort.and_then(|chosen| effort(agent, chosen));
    let mut args = Vec::new();
    if let Resume::Subcommand(subcommand) = flags.resume {
        args.extend(run.resume_id.map(|_| subcommand.to_owned()));
    }
    if let Some(model) = model {
        args.extend(["--model".to_owned(), model.to_owned()]);
    }
    if let (Some(effort), Some([flag, value])) = (effort, flags.effort) {
        args.extend([flag.to_owned(), value.replace("{}", effort)]);
    }
    args.extend(permission_args(agent, run.permission_mode));
    if model.is_some() || effort.is_some() {
        args.extend(flags.with_choices.iter().map(|arg| (*arg).to_owned()));
    }
    if let Some(resume_id) = run.resume_id {
        if let Resume::Flag(flag) = flags.resume {
            args.push(flag.to_owned());
        }
        args.push(resume_id.to_owned());
    }
    args
}

/// The session a terminal agent's command resumes, read from its arguments.
pub(crate) fn resumed_session<'a>(provider: &str, args: &'a [String]) -> Option<&'a str> {
    match agent_catalog::agent(provider)?.terminal.resume {
        Resume::Subcommand(subcommand) => (args.first()? == subcommand)
            .then(|| args.last().map(String::as_str))
            .flatten(),
        Resume::Flag(flag) => args
            .windows(2)
            .find(|pair| pair[0] == flag)
            .map(|pair| pair[1].as_str()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(name: &str) -> &'static AgentEntry {
        agent_catalog::agent(name).unwrap()
    }

    fn codex() -> &'static AgentEntry {
        agent("codex")
    }

    fn claude() -> &'static AgentEntry {
        agent("claude")
    }

    fn run<'a>(
        resume_id: Option<&'a str>,
        mode: &str,
        model: Option<&'a str>,
        effort: Option<&'a str>,
    ) -> TerminalRun<'a> {
        TerminalRun {
            resume_id,
            permission_mode: PermissionMode::parse(mode).unwrap(),
            model,
            effort,
        }
    }

    #[test]
    fn codex_and_claude_have_only_normal_and_yolo() {
        assert_eq!(
            permission_args(codex(), PermissionMode::WorkspaceWrite),
            ["--sandbox", "workspace-write"]
        );
        assert_eq!(
            permission_args(codex(), PermissionMode::Bypass),
            ["--dangerously-bypass-approvals-and-sandbox"]
        );
        assert_eq!(
            permission_args(claude(), PermissionMode::WorkspaceWrite),
            ["--permission-mode", "acceptEdits"]
        );
        assert_eq!(
            permission_args(claude(), PermissionMode::Bypass),
            ["--dangerously-skip-permissions"]
        );
        assert!(permission_args(agent("pi"), PermissionMode::Bypass).is_empty());
    }

    #[test]
    fn claude_and_codex_take_model_effort_mode_and_resume() {
        assert_eq!(
            args(
                "claude",
                &run(
                    Some("session-1"),
                    "workspace-write",
                    Some(" sonnet "),
                    Some("high")
                )
            ),
            [
                "--model",
                "sonnet",
                "--effort",
                "high",
                "--permission-mode",
                "acceptEdits",
                "--resume",
                "session-1"
            ]
        );
        assert_eq!(
            args(
                "codex",
                &run(
                    Some("session-2"),
                    "workspace-write",
                    Some("gpt-5.6-codex"),
                    Some("xhigh")
                )
            ),
            [
                "resume",
                "--model",
                "gpt-5.6-codex",
                "--config",
                "model_reasoning_effort=\"xhigh\"",
                "--sandbox",
                "workspace-write",
                "session-2"
            ]
        );
    }

    #[test]
    fn each_cli_gets_only_the_flags_it_has() {
        assert_eq!(
            args(
                "hermes",
                &run(
                    None,
                    "workspace-write",
                    Some("anthropic/claude-sonnet-4.6"),
                    Some("ultra")
                )
            ),
            [
                "--model",
                "anthropic/claude-sonnet-4.6",
                "--reasoning",
                "ultra",
                "--tui"
            ]
        );
        assert_eq!(
            args(
                "pi",
                &run(
                    None,
                    "workspace-write",
                    Some("anthropic/claude-sonnet-4.6"),
                    Some("ultra")
                )
            ),
            [
                "--model",
                "anthropic/claude-sonnet-4.6",
                "--thinking",
                "max"
            ]
        );
        assert_eq!(
            args(
                "opencode",
                &run(None, "workspace-write", Some("openai/gpt-5"), Some("high"))
            ),
            ["--model", "openai/gpt-5"]
        );
        assert_eq!(
            args(
                "omp",
                &run(
                    Some("/sessions/omp.jsonl"),
                    "bypass",
                    Some("openai-codex/gpt-5.6-sol"),
                    Some("xhigh")
                )
            ),
            [
                "--model",
                "openai-codex/gpt-5.6-sol",
                "--thinking",
                "xhigh",
                "--approval-mode",
                "yolo",
                "--resume",
                "/sessions/omp.jsonl"
            ]
        );
        assert_eq!(
            args(
                "grok",
                &run(
                    Some("018f0000-0000-7000-8000-000000000000"),
                    "bypass",
                    Some("grok-4.5"),
                    Some("max")
                )
            ),
            [
                "--model",
                "grok-4.5",
                "--reasoning-effort",
                "max",
                "--permission-mode",
                "bypassPermissions",
                "--resume",
                "018f0000-0000-7000-8000-000000000000"
            ]
        );
        assert_eq!(effort(codex(), "ultra"), Some("max"));
        assert_eq!(effort(agent("opencode"), "high"), None);
    }
}
