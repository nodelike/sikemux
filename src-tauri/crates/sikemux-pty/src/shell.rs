use std::path::{Path, PathBuf};

use portable_pty::CommandBuilder;

use crate::error::{PtyError, PtyResult};
use crate::launch::PtyContext;

/// Execute startup commands before the interactive shell is launched. Sending
/// text to readline makes it visible in the terminal (and multi-line commands
/// are especially fragile), so startup must be a shell argument, never PTY
/// input. Once it returns, replace the bootstrap shell with the normal local
/// shell so users always land at a usable prompt.
#[cfg(unix)]
pub fn startup_bootstrap(startup: &str, login_shell: bool) -> String {
    let login = if login_shell { " -l" } else { "" };
    format!("{startup}\nexec \"$SIKEMUX_SHELL\"{login}")
}

/// Shells launched without an injected rc file need `-l` to read their profile,
/// the way a normal terminal emulator starts them. Shells that do get an
/// injected rc file are already reading the user's chain through it.
#[cfg(unix)]
pub fn shell_wants_login_flag(shell: &str) -> bool {
    matches!(detect_shell_kind(shell), Some(ShellKind::Zsh))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellKind {
    Zsh,
    Bash,
    Fish,
    PowerShell,
}

/// Lifetime guard for startup files. Fish and PowerShell use argv hooks and
/// therefore have no directory, but still return a guard to mark parsing active.
pub struct ShellLaunchIntegration {
    files: Option<IntegrationFiles>,
}

enum IntegrationFiles {
    Created(tempfile::TempDir),
    Adopted(AdoptedDirectory),
}

/// A startup directory made by an earlier process, removed when dropped.
struct AdoptedDirectory(PathBuf);

impl Drop for AdoptedDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl ShellLaunchIntegration {
    /// Where the startup files live, if this shell has any.
    pub fn files_directory(&self) -> Option<&Path> {
        match self.files.as_ref()? {
            IntegrationFiles::Created(directory) => Some(directory.path()),
            IntegrationFiles::Adopted(directory) => Some(&directory.0),
        }
    }

    /// Takes over the guard of a shell that another process launched.
    pub fn adopt(files_directory: Option<PathBuf>) -> Self {
        Self {
            files: files_directory.map(|path| IntegrationFiles::Adopted(AdoptedDirectory(path))),
        }
    }
}

const BASH_INTEGRATION: &str = r#"# Sikemux ephemeral shell integration; generated per PTY.
[[ -r "$HOME/.bashrc" ]] && source "$HOME/.bashrc"

__sikemux_emit_cwd() {
  local path="${PWD//[[:cntrl:]]/}"
  path="${path//%/%25}"
  path="${path//\\/%5C}"
  path="${path// /%20}"
  path="${path//#/%23}"
  path="${path//\?/%3F}"
  builtin printf '\e]7;file://localhost%s\a' "$path"
}
__sikemux_prompt() {
  local command_status=$?
  builtin printf '\e]133;D;%d\a' "$command_status"
  __sikemux_emit_cwd
  builtin printf '\e]133;A\a'
  return "$command_status"
}
case "$(declare -p PROMPT_COMMAND 2>/dev/null)" in
  "declare -a"*) PROMPT_COMMAND=(__sikemux_prompt "${PROMPT_COMMAND[@]}") ;;
  *) PROMPT_COMMAND="__sikemux_prompt${PROMPT_COMMAND:+;$PROMPT_COMMAND}" ;;
esac
if (( BASH_VERSINFO[0] > 4 || (BASH_VERSINFO[0] == 4 && BASH_VERSINFO[1] >= 4) )); then
  PS0="${PS0-}"$'\e]133;B\a\e]133;C\a'
else
  PS1="${PS1-}"'\[\e]133;B\a\]'
fi
"#;

const FISH_INTEGRATION: &str = r#"# Sikemux post-config init command; no config paths are replaced.
function __sikemux_emit_cwd
    set -l path (string replace -ar '[[:cntrl:]]' '' -- "$PWD")
    set path (string replace -a '%' '%25' -- "$path")
    set path (string replace -a '\\' '%5C' -- "$path")
    set path (string replace -a ' ' '%20' -- "$path")
    set path (string replace -a '#' '%23' -- "$path")
    set path (string replace -a '?' '%3F' -- "$path")
    printf '\e]7;file://localhost%s\a' "$path"
end
function __sikemux_prompt --on-event fish_prompt
    set -l command_status $status
    __sikemux_emit_cwd
    printf '\e]133;A\a'
    return $command_status
end
function __sikemux_preexec --on-event fish_preexec
    printf '\e]133;B\a\e]133;C\a'
end
function __sikemux_postexec --on-event fish_postexec
    set -l command_status $status
    printf '\e]133;D;%d\a' $command_status
    return $command_status
end
"#;

const POWERSHELL_INTEGRATION: &str = r#"$global:__sikemux_original_prompt = $function:global:prompt
if ($null -eq $global:__sikemux_original_prompt) {
    $global:__sikemux_original_prompt = { "PS $($ExecutionContext.SessionState.Path.CurrentLocation)> " }
}
function global:prompt {
    $sikemux_ok = $?
    $sikemux_text = & $global:__sikemux_original_prompt
    $sikemux_code = if ($sikemux_ok) { 0 } else { 1 }
    [Console]::Write("$([char]27)]133;D;$sikemux_code$([char]7)")
    try {
        $sikemux_path = $ExecutionContext.SessionState.Path.CurrentFileSystemLocation.Path
        $sikemux_encoded = [Uri]::EscapeDataString([string]$sikemux_path).Replace('%2F', '/').Replace('%5C', '/')
        if (-not $sikemux_encoded.StartsWith('/')) { $sikemux_encoded = '/' + $sikemux_encoded }
        [Console]::Write("$([char]27)]7;file://localhost$sikemux_encoded$([char]7)")
    } catch {}
    [Console]::Write("$([char]27)]133;A$([char]7)")
    return "$sikemux_text$([char]27)]133;B$([char]7)"
}"#;

pub fn detect_shell_kind(shell: &str) -> Option<ShellKind> {
    let name = shell.rsplit(['/', '\\']).next()?.to_ascii_lowercase();
    let name = name.strip_suffix(".exe").unwrap_or(&name);
    match name {
        "zsh" => Some(ShellKind::Zsh),
        "bash" => Some(ShellKind::Bash),
        "fish" => Some(ShellKind::Fish),
        "powershell" | "pwsh" => Some(ShellKind::PowerShell),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TaskShellPlatform {
    Unix,
    Windows,
}

const CURRENT_TASK_SHELL_PLATFORM: TaskShellPlatform = if cfg!(windows) {
    TaskShellPlatform::Windows
} else {
    TaskShellPlatform::Unix
};

fn task_shell_arguments(
    shell: &str,
    command: &str,
    platform: TaskShellPlatform,
) -> PtyResult<Vec<String>> {
    if matches!(detect_shell_kind(shell), Some(ShellKind::PowerShell)) {
        return Ok(vec![
            "-NoLogo".into(),
            "-NonInteractive".into(),
            "-Command".into(),
            command.into(),
        ]);
    }
    if platform == TaskShellPlatform::Unix
        || matches!(
            detect_shell_kind(shell),
            Some(ShellKind::Zsh | ShellKind::Bash | ShellKind::Fish)
        )
    {
        return Ok(vec!["-c".into(), command.into()]);
    }
    let executable = shell
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if matches!(executable.as_str(), "cmd" | "cmd.exe") {
        return Ok(vec!["/D".into(), "/S".into(), "/C".into(), command.into()]);
    }
    Err(PtyError::BadArg(
        "configured shell does not support task execution",
    ))
}

pub fn configure_task_command(
    command: &mut CommandBuilder,
    shell: &str,
    task: &str,
) -> PtyResult<()> {
    command.args(task_shell_arguments(
        shell,
        task,
        CURRENT_TASK_SHELL_PLATFORM,
    )?);
    Ok(())
}

pub fn shell_integration_requested(
    context: Option<&PtyContext>,
    has_startup: bool,
    inherited_ssh: bool,
) -> bool {
    let Some(context) = context else {
        return false;
    };
    context.shell_integration
        && !has_startup
        && !inherited_ssh
        && context.agent_id.is_none()
        && context.agent_type.is_none()
        && matches!(context.session_kind.as_str(), "project" | "command")
}

pub fn inherited_ssh_environment() -> bool {
    ["SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY"]
        .iter()
        .any(|key| std::env::var_os(key).is_some_and(|value| !value.is_empty()))
}

fn temporary_shell_file(
    relative: &Path,
    contents: &str,
) -> std::io::Result<(tempfile::TempDir, PathBuf)> {
    let directory = temporary_shell_directory()?;
    let path = write_temporary_shell_file(&directory, relative, contents)?;
    Ok((directory, path))
}

fn temporary_shell_directory() -> std::io::Result<tempfile::TempDir> {
    tempfile::Builder::new().prefix("sikemux-shell-").tempdir()
}

fn write_temporary_shell_file(
    directory: &tempfile::TempDir,
    relative: &Path,
    contents: &str,
) -> std::io::Result<PathBuf> {
    let path = directory.path().join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, contents)?;
    Ok(path)
}

pub fn configure_shell_integration(
    cmd: &mut CommandBuilder,
    shell: &str,
) -> std::io::Result<Option<ShellLaunchIntegration>> {
    let Some(kind) = detect_shell_kind(shell) else {
        return Ok(None);
    };
    match kind {
        // zsh gets no integration: it is driven by ZDOTDIR, and pointing that
        // at a scratch directory silently redirects everything a user's config
        // derives from it — `HISTFILE` above all, so a session's history was
        // written into a temp dir and deleted on exit. zsh is launched exactly
        // as Terminal, Ghostty and Alacritty launch it, and the cwd/exit
        // metadata is given up rather than paid for with the user's shell.
        ShellKind::Zsh => Ok(None),
        ShellKind::Bash => {
            let (directory, path) = temporary_shell_file(Path::new("bashrc"), BASH_INTEGRATION)?;
            cmd.env("SIKEMUX_SHELL_INTEGRATION", "1");
            cmd.arg("--rcfile");
            cmd.arg(path);
            Ok(Some(ShellLaunchIntegration {
                files: Some(IntegrationFiles::Created(directory)),
            }))
        }
        ShellKind::Fish => {
            cmd.env("SIKEMUX_SHELL_INTEGRATION", "1");
            // Fish's native init command runs after its normal configuration
            // chain, preserving user/vendor conf.d scripts and autoload paths.
            cmd.args(["--init-command", FISH_INTEGRATION]);
            Ok(Some(ShellLaunchIntegration { files: None }))
        }
        ShellKind::PowerShell => {
            cmd.env("SIKEMUX_SHELL_INTEGRATION", "1");
            cmd.args(["-NoExit", "-Command", POWERSHELL_INTEGRATION]);
            Ok(Some(ShellLaunchIntegration { files: None }))
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::shell_wants_login_flag;
    #[cfg(unix)]
    use super::startup_bootstrap;
    use super::{
        configure_shell_integration, configure_task_command, detect_shell_kind,
        shell_integration_requested, task_shell_arguments, ShellKind, TaskShellPlatform,
    };
    use crate::tests::{env, local_shell_context};
    use portable_pty::CommandBuilder;

    #[test]
    fn task_shell_uses_direct_arguments_without_requoting_or_interactive_flags() {
        let task = "printf '%s' \"a b;$TOKEN\"";
        assert_eq!(
            task_shell_arguments("/bin/zsh", task, TaskShellPlatform::Unix).expect("zsh task args"),
            ["-c", task]
        );
        assert_eq!(
            task_shell_arguments("pwsh.exe", task, TaskShellPlatform::Windows)
                .expect("PowerShell task args"),
            ["-NoLogo", "-NonInteractive", "-Command", task]
        );
        assert_eq!(
            task_shell_arguments("cmd.exe", task, TaskShellPlatform::Windows)
                .expect("cmd task args"),
            ["/D", "/S", "/C", task]
        );
        assert!(task_shell_arguments("custom.exe", task, TaskShellPlatform::Windows).is_err());

        let mut command = CommandBuilder::new("/bin/zsh");
        configure_task_command(&mut command, "/bin/zsh", task).expect("configure task");
        let argv: Vec<String> = command
            .get_argv()
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();
        assert_eq!(argv, ["/bin/zsh", "-c", task]);
        assert_eq!(env(&command, "SIKEMUX_SHELL_INTEGRATION"), None);
        assert!(!argv
            .iter()
            .any(|argument| argument == "-i" || argument == "-NoExit"));
    }

    #[test]
    fn shell_integration_is_strictly_opt_in_and_local_interactive_only() {
        let mut context = local_shell_context();
        assert!(shell_integration_requested(Some(&context), false, false));

        context.shell_integration = false;
        assert!(!shell_integration_requested(Some(&context), false, false));
        context.shell_integration = true;
        assert!(!shell_integration_requested(Some(&context), true, false));
        assert!(!shell_integration_requested(Some(&context), false, true));

        context.session_kind = "ssh".into();
        assert!(!shell_integration_requested(Some(&context), false, false));
        context.session_kind = "project".into();
        context.agent_id = Some("agent-1".into());
        assert!(!shell_integration_requested(Some(&context), false, false));
        context.agent_id = None;
        context.agent_type = Some("codex".into());
        assert!(!shell_integration_requested(Some(&context), false, false));
        assert!(!shell_integration_requested(None, false, false));
    }

    #[test]
    fn shell_detection_claims_only_exact_supported_executables() {
        assert_eq!(detect_shell_kind("/bin/zsh"), Some(ShellKind::Zsh));
        assert_eq!(detect_shell_kind("bash"), Some(ShellKind::Bash));
        assert_eq!(
            detect_shell_kind("/opt/homebrew/bin/fish"),
            Some(ShellKind::Fish)
        );
        assert_eq!(
            detect_shell_kind(r"C:\Program Files\PowerShell\7\pwsh.exe"),
            Some(ShellKind::PowerShell)
        );
        assert_eq!(
            detect_shell_kind("powershell.exe"),
            Some(ShellKind::PowerShell)
        );
        assert_eq!(detect_shell_kind("/bin/sh"), None);
        assert_eq!(detect_shell_kind("/usr/local/bin/my-zsh-wrapper"), None);
    }

    #[test]
    fn supported_shells_receive_ephemeral_hooks_without_dotfile_writes() {
        // zsh is deliberately left alone: hooking it means owning ZDOTDIR, and
        // a user's config derives real paths from that.
        let mut zsh = CommandBuilder::new("/bin/zsh");
        assert!(configure_shell_integration(&mut zsh, "/bin/zsh")
            .expect("configure zsh")
            .is_none());
        assert_eq!(env(&zsh, "ZDOTDIR"), None);
        assert_eq!(env(&zsh, "SIKEMUX_SHELL_INTEGRATION"), None);
        #[cfg(unix)]
        assert!(shell_wants_login_flag("/bin/zsh"));

        let mut bash = CommandBuilder::new("/bin/bash");
        let bash_guard = configure_shell_integration(&mut bash, "/bin/bash")
            .expect("configure bash")
            .expect("supported bash");
        let bash_args: Vec<String> = bash
            .get_argv()
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(bash_args.get(1).map(String::as_str), Some("--rcfile"));
        let bash_hook = std::fs::read_to_string(&bash_args[2]).expect("read bash hook");
        assert!(bash_hook.contains("PROMPT_COMMAND"));
        assert!(bash_hook.contains("PS0="));

        let mut fish = CommandBuilder::new("fish");
        let original_fish_xdg = env(&fish, "XDG_CONFIG_HOME");
        let fish_guard = configure_shell_integration(&mut fish, "fish")
            .expect("configure fish")
            .expect("supported fish");
        assert_eq!(env(&fish, "XDG_CONFIG_HOME"), original_fish_xdg);
        let fish_args: Vec<String> = fish
            .get_argv()
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(fish_args.get(1).map(String::as_str), Some("--init-command"));
        assert!(fish_args[2].contains("fish_preexec"));
        assert!(fish_args[2].contains("fish_postexec"));
        assert!(!fish_args[2].contains("XDG_CONFIG_HOME"));

        let mut powershell = CommandBuilder::new("pwsh");
        let powershell_guard = configure_shell_integration(&mut powershell, "pwsh")
            .expect("configure PowerShell")
            .expect("supported PowerShell");
        let powershell_args: Vec<String> = powershell
            .get_argv()
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            &powershell_args[1..3],
            &["-NoExit".to_string(), "-Command".to_string()]
        );
        assert!(powershell_args[3].contains("function global:prompt"));

        let mut unsupported = CommandBuilder::new("/bin/sh");
        assert!(configure_shell_integration(&mut unsupported, "/bin/sh")
            .expect("unsupported shell is not an error")
            .is_none());
        assert_eq!(unsupported.get_argv().len(), 1);
        assert_eq!(env(&unsupported, "SIKEMUX_SHELL_INTEGRATION"), None);

        drop((bash_guard, fish_guard, powershell_guard));
    }

    #[cfg(unix)]
    #[test]
    fn startup_runs_before_the_interactive_shell_without_pty_input() {
        let bootstrap = startup_bootstrap("ssh prod-db", false);
        assert_eq!(bootstrap, "ssh prod-db\nexec \"$SIKEMUX_SHELL\"");
        assert!(!bootstrap.contains('\r'));

        // A shell that reads its profile keeps doing so after the startup
        // command hands over, or the user lands in a stripped environment.
        let login = startup_bootstrap("ssh prod-db", true);
        assert_eq!(login, "ssh prod-db\nexec \"$SIKEMUX_SHELL\" -l");
    }

    #[cfg(unix)]
    #[test]
    fn task_command_runs_noninteractive_with_exact_cwd_env_output_and_status() {
        use portable_pty::{NativePtySystem, PtySize, PtySystem};
        use std::io::Read;

        let root = tempfile::tempdir().expect("task root");
        let cwd = root.path().join("project with spaces");
        std::fs::create_dir(&cwd).expect("task cwd");
        let pair = NativePtySystem::default()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("open task pty");
        let mut command = CommandBuilder::new("/bin/sh");
        command.cwd(&cwd);
        command.env("TASK_TEST_VALUE", "value with spaces");
        configure_task_command(
            &mut command,
            "/bin/sh",
            "printf '%s|%s' \"$PWD\" \"$TASK_TEST_VALUE\"; exit 7",
        )
        .expect("configure task command");
        let mut reader = pair.master.try_clone_reader().expect("clone task reader");
        let mut child = pair
            .slave
            .spawn_command(command)
            .expect("spawn task command");
        drop(pair.slave);

        let mut bytes = Vec::new();
        let mut chunk = [0u8; 512];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(read) => bytes.extend_from_slice(&chunk[..read]),
                Err(_) => break,
            }
        }
        let status = child.wait().expect("wait for task command");
        let output = String::from_utf8_lossy(&bytes);
        assert_eq!(status.exit_code(), 7);
        assert!(output.contains(cwd.to_string_lossy().as_ref()));
        assert!(output.contains("value with spaces"));
    }
}
