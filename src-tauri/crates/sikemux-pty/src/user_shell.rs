use std::collections::HashMap;
#[cfg(unix)]
use std::process::Command;
use std::sync::OnceLock;

/// Opens the payload, fencing it off from whatever an interactive rc file
/// prints on the way past — banners, `clear`, prompt-init escapes. Only bytes
/// after the last occurrence are parsed, so an rc file that echoes the command
/// line back cannot inject entries.
#[cfg(unix)]
const LOGIN_ENV_SENTINEL: &str = "@@SIKEMUX_ENV@@";

/// Closes the payload. `env -0` terminates its last record with a NUL, so
/// anything a `zshexit`/`precmd` hook prints afterwards would otherwise become
/// a trailing record — and parse as a real entry if it happened to contain an
/// `=`. Fencing both ends means only what the capture itself emitted is read.
#[cfg(unix)]
const LOGIN_ENV_SENTINEL_END: &str = "@@SIKEMUX_ENV_END@@";

/// A profile that blocks forever must never stop the app from starting. The
/// capture runs on its own thread and drains the pipe, so the child cannot
/// deadlock on a full one; past this deadline we launch with what we have.
#[cfg(unix)]
const LOGIN_ENV_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Keys the PTY layer owns. Importing these would let a profile rename the
/// terminal, redirect shell integration, or hand a pane the capture shell's
/// own working directory and launchd-scoped temp dir. `PATH` is excluded
/// because `fix_path_from_login_shell` already unions it process-wide.
///
/// This list is about keys whose *values* would be wrong in a pane, not about
/// identity: `configure_pty_environment` scrubs `OPTIONAL_PTY_ENV` after the
/// fill, and that is what stops a profile forging a pane's identity.
///
/// `EDITOR`/`VISUAL` are deliberately absent — the PTY layer already installs
/// its own editor only when neither is set, so importing them keeps a user who
/// exports one in their profile from getting different behaviour depending on
/// whether the app was launched from a terminal or from the Dock.
#[cfg(unix)]
const LOGIN_ENV_SIKEMUX_OWNED: &[&str] = &[
    "COLORTERM",
    "COLUMNS",
    "LINES",
    "OLDPWD",
    "PATH",
    "PWD",
    "SHELL",
    "SHLVL",
    "TERM",
    "TERM_PROGRAM",
    "TERM_PROGRAM_VERSION",
    "TERM_SESSION_ID",
    "TMPDIR",
    "ZDOTDIR",
    "_",
];

/// What the user's shell profile exports, captured once per app run.
///
/// A macOS GUI app inherits launchd's environment, never the user's profile.
/// `fix_path_from_login_shell` recovers `PATH` from it; this recovers the
/// rest. That matters most for a PTY launched with a direct command — an agent
/// CLI running as the PTY's own process with no shell in between — because it
/// reads no profile at all. An API key or token the user keeps in `.zshrc` is
/// simply absent there, so the CLI comes up asking them to log in while the
/// very same CLI works in their own terminal.
///
/// `-i` is the load-bearing flag: zsh sources `.zshrc` only when interactive,
/// and that is where people put exports. `-l` alone reads `.zprofile` and
/// misses them, which is why the `PATH` capture above cannot be reused.
pub fn login_shell_environment() -> &'static HashMap<String, String> {
    #[cfg(unix)]
    {
        &login_shell_capture().environment
    }
    // Windows desktop apps already inherit the user's environment.
    #[cfg(windows)]
    {
        static EMPTY: OnceLock<HashMap<String, String>> = OnceLock::new();
        EMPTY.get_or_init(HashMap::new)
    }
}

/// The locale variables from `login_shell_environment`, for processes that
/// otherwise get only the app's own environment, which has no locale.
pub fn login_shell_locale() -> impl Iterator<Item = (&'static String, &'static String)> {
    login_shell_environment()
        .iter()
        .filter(|(key, _)| *key == "LANG" || key.starts_with("LC_"))
}

/// What one run of the user's login shell told us: the `PATH` it resolves and
/// everything else it exports. Both come out of the same `env -0` payload, so
/// startup pays for one interactive shell rather than two.
#[cfg(unix)]
#[derive(Default)]
struct LoginShellCapture {
    path: Option<String>,
    environment: HashMap<String, String>,
}

/// The `PATH` the user's login shell resolves, if the capture produced one.
#[cfg(unix)]
pub fn login_shell_path() -> Option<&'static str> {
    login_shell_capture().path.as_deref()
}

#[cfg(unix)]
fn login_shell_capture() -> &'static LoginShellCapture {
    static CACHE: OnceLock<LoginShellCapture> = OnceLock::new();
    CACHE.get_or_init(capture_login_shell)
}

/// Populate the `login_shell_environment` cache from the startup thread.
///
/// `fix_path_from_login_shell` already filled it — both halves come out of one
/// capture — so this is free once that has run. It stays as its own call
/// because the cache is first *needed* inside `pty_spawn`, an async command,
/// and initialising it there would park an async runtime worker for up to
/// `LOGIN_ENV_TIMEOUT` with concurrent spawns queued behind it.
pub fn warm_login_shell_environment() {
    let _ = login_shell_environment();
}

#[cfg(unix)]
fn capture_login_shell() -> LoginShellCapture {
    let shell = configured_shell();
    // `env -0` rather than newline records: a value may contain a newline, but
    // never a NUL.
    let script =
        format!("printf %s '{LOGIN_ENV_SENTINEL}'; env -0; printf %s '{LOGIN_ENV_SENTINEL_END}'");
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        // The user's environment is read from this shell, so it cannot wait for it.
        #[allow(clippy::disallowed_methods)]
        let mut command = Command::new(&shell);
        command
            .args(["-l", "-i", "-c", &script])
            // An rc file that reads stdin sees EOF instead of blocking.
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        for key in crate::launch::OPTIONAL_PTY_ENV {
            command.env_remove(key);
        }
        let _ = sender.send(command.output().ok());
    });
    // On timeout the capture thread stays parked in `output()` until the shell
    // it is waiting on exits, so a profile that blocks forever leaks one thread
    // and one process for the life of the app. That is bounded — this runs
    // exactly once — and the alternative is process-group teardown for a case
    // that ends the moment the user fixes their profile.
    let mut capture = match receiver.recv_timeout(LOGIN_ENV_TIMEOUT) {
        Ok(Some(output)) if output.status.success() => LoginShellCapture {
            path: parse_login_shell_path(&output.stdout),
            environment: parse_login_shell_environment(&output.stdout),
        },
        _ => LoginShellCapture::default(),
    };
    ensure_utf8_locale(&mut capture.environment);
    capture
}

/// Terminals set a UTF-8 locale when the shell leaves it unset, and tools
/// such as Ruby fall back to ASCII without one. macOS's own zsh profile picks
/// `C.UTF-8`, so a shell that sets nothing gets the same.
#[cfg(unix)]
fn ensure_utf8_locale(environment: &mut HashMap<String, String>) {
    let has_locale = ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .any(|key| environment.get(*key).is_some_and(|value| !value.is_empty()));
    if !has_locale {
        environment.insert("LANG".to_string(), "C.UTF-8".to_string());
    }
}

/// The fenced `env -0` payload, split into key/value records.
///
/// Work on bytes, not `from_utf8_lossy`: a value that is not valid UTF-8 would
/// otherwise have replacement characters substituted into it and be read in
/// corrupted form, which is a silent way to break a token. Each record is
/// converted individually so one bad value drops itself instead of the whole
/// capture.
#[cfg(unix)]
fn login_shell_records(stdout: &[u8]) -> impl Iterator<Item = (&str, &str)> {
    let payload = login_shell_payload(stdout).unwrap_or(&[]);
    payload
        .split(|byte| *byte == 0)
        .filter_map(|record| std::str::from_utf8(record).ok())
        .filter_map(|record| record.split_once('='))
        .filter(|(key, _)| !key.is_empty() && !key.contains(char::is_whitespace))
}

#[cfg(unix)]
fn login_shell_payload(stdout: &[u8]) -> Option<&[u8]> {
    let start = rfind_bytes(stdout, LOGIN_ENV_SENTINEL.as_bytes())? + LOGIN_ENV_SENTINEL.len();
    let payload = &stdout[start..];
    // Without the closing fence we cannot tell the payload from whatever an
    // exit hook printed after it, so refuse rather than guess.
    let end = rfind_bytes(payload, LOGIN_ENV_SENTINEL_END.as_bytes())?;
    Some(&payload[..end])
}

#[cfg(unix)]
fn parse_login_shell_environment(stdout: &[u8]) -> HashMap<String, String> {
    login_shell_records(stdout)
        .filter(|(key, _)| !LOGIN_ENV_SIKEMUX_OWNED.contains(key))
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

#[cfg(unix)]
fn parse_login_shell_path(stdout: &[u8]) -> Option<String> {
    login_shell_records(stdout)
        .filter(|(key, _)| *key == "PATH")
        .map(|(_, value)| value.trim().to_string())
        .last()
        .filter(|path| !path.is_empty())
}

#[cfg(unix)]
fn rfind_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .rposition(|window| window == needle)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShellPlatform {
    Unix,
    Windows,
}

const CURRENT_SHELL_PLATFORM: ShellPlatform = if cfg!(windows) {
    ShellPlatform::Windows
} else {
    ShellPlatform::Unix
};

fn resolve_configured_shell(
    platform: ShellPlatform,
    sikemux_shell: Option<&str>,
    unix_shell: Option<&str>,
) -> String {
    match platform {
        ShellPlatform::Unix => unix_shell.unwrap_or("/bin/zsh"),
        ShellPlatform::Windows => sikemux_shell.unwrap_or("powershell.exe"),
    }
    .to_string()
}

/// The executable a newly spawned interactive PTY will use. Keep health
/// reporting and PTY launch on this one platform policy so the frontend never
/// configures integration for a different shell than native will execute.
pub fn configured_shell() -> String {
    let sikemux_shell = std::env::var("SIKEMUX_SHELL").ok();
    let unix_shell = std::env::var("SHELL").ok();
    resolve_configured_shell(
        CURRENT_SHELL_PLATFORM,
        sikemux_shell.as_deref(),
        unix_shell.as_deref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `PATH` and the profile environment come out of the same capture, and
    /// only what the fenced payload holds counts — an rc banner that mentions
    /// a PATH before the sentinel is chatter, not the shell's answer.
    #[cfg(unix)]
    #[test]
    fn login_shell_path_comes_from_the_fenced_payload() {
        let stdout = format!(
            "welcome\n\u{1b}]7;file://host/tmp\u{7}PATH=/decoy\n{}HOME=/Users/x\0PATH=/Users/test/.mise/shims:/usr/bin\0{}prompt noise",
            super::LOGIN_ENV_SENTINEL,
            super::LOGIN_ENV_SENTINEL_END
        );

        assert_eq!(
            parse_login_shell_path(stdout.as_bytes()).as_deref(),
            Some("/Users/test/.mise/shims:/usr/bin")
        );
        // The same payload still keeps PATH out of the imported environment.
        assert!(!parse_login_shell_environment(stdout.as_bytes()).contains_key("PATH"));
    }

    #[cfg(unix)]
    #[test]
    fn login_shell_path_is_absent_without_a_fenced_payload() {
        assert!(parse_login_shell_path(b"PATH=/decoy\0").is_none());
        let truncated = format!("{}PATH=/decoy\0", super::LOGIN_ENV_SENTINEL);
        assert!(parse_login_shell_path(truncated.as_bytes()).is_none());
    }

    #[test]
    fn configured_shell_resolver_preserves_unix_shell_and_default() {
        assert_eq!(
            resolve_configured_shell(
                ShellPlatform::Unix,
                Some("ignored-windows-override"),
                Some("/opt/homebrew/bin/fish"),
            ),
            "/opt/homebrew/bin/fish"
        );
        assert_eq!(
            resolve_configured_shell(ShellPlatform::Unix, None, None),
            "/bin/zsh"
        );
    }

    #[test]
    fn configured_shell_resolver_gives_windows_override_precedence() {
        assert_eq!(
            resolve_configured_shell(
                ShellPlatform::Windows,
                Some(r"C:\Program Files\PowerShell\7\pwsh.exe"),
                Some("ignored-unix-shell"),
            ),
            r"C:\Program Files\PowerShell\7\pwsh.exe"
        );
        assert_eq!(
            resolve_configured_shell(ShellPlatform::Windows, None, Some("ignored-unix-shell")),
            "powershell.exe"
        );
    }

    /// Interactive rc files print banners, run `clear`, and emit prompt-init
    /// escapes before the payload ever appears. Only what follows the sentinel
    /// is environment.
    #[cfg(unix)]
    #[test]
    fn login_env_parse_discards_interactive_rc_chatter_before_the_sentinel() {
        let stdout = format!(
            "\u{1b}[2J\u{1b}[Hbanner line\nANTHROPIC_API_KEY=decoy\n{}HOME=/Users/x\0ANTHROPIC_API_KEY=sk-real\0{}",
            super::LOGIN_ENV_SENTINEL,
            super::LOGIN_ENV_SENTINEL_END
        );

        let parsed = super::parse_login_shell_environment(stdout.as_bytes());

        assert_eq!(
            parsed.get("ANTHROPIC_API_KEY").map(String::as_str),
            Some("sk-real")
        );
        assert_eq!(parsed.get("HOME").map(String::as_str), Some("/Users/x"));
        assert_eq!(parsed.len(), 2);
    }

    /// An rc file that echoes the capture command back would otherwise let a
    /// second sentinel smuggle entries in; the last one always wins.
    #[cfg(unix)]
    #[test]
    fn login_env_parse_honours_only_the_final_sentinel() {
        let sentinel = super::LOGIN_ENV_SENTINEL;
        let end = super::LOGIN_ENV_SENTINEL_END;
        let stdout = format!("{sentinel}INJECTED=yes\0{sentinel}REAL=yes\0{end}");

        let parsed = super::parse_login_shell_environment(stdout.as_bytes());

        assert_eq!(parsed.get("REAL").map(String::as_str), Some("yes"));
        assert!(!parsed.contains_key("INJECTED"));
    }

    /// `env -0` ends its last record with a NUL, so anything a `zshexit` or
    /// `precmd` hook prints after the payload arrives as a trailing record —
    /// and parses as an entry if it contains an `=`. The closing fence is what
    /// keeps a hook from injecting one.
    #[cfg(unix)]
    #[test]
    fn login_env_parse_ignores_anything_printed_after_the_payload() {
        let stdout = format!(
            "{}REAL=yes\0{}\nrestored session: TRAILING=injected\n",
            super::LOGIN_ENV_SENTINEL,
            super::LOGIN_ENV_SENTINEL_END
        );

        let parsed = super::parse_login_shell_environment(stdout.as_bytes());

        assert_eq!(parsed.get("REAL").map(String::as_str), Some("yes"));
        assert!(!parsed.contains_key("TRAILING"));
        assert_eq!(parsed.len(), 1);
    }

    /// A value that is not valid UTF-8 must drop itself rather than be imported
    /// with replacement characters substituted in — a corrupted token is worse
    /// than an absent one. Its neighbours still arrive intact.
    #[cfg(unix)]
    #[test]
    fn login_env_parse_drops_non_utf8_records_without_corrupting_them() {
        let mut stdout = Vec::new();
        stdout.extend_from_slice(super::LOGIN_ENV_SENTINEL.as_bytes());
        stdout.extend_from_slice(b"GOOD=yes\0BAD=");
        stdout.extend_from_slice(&[0xff, 0xfe]);
        stdout.extend_from_slice(b"\0ALSO_GOOD=yes\0");
        stdout.extend_from_slice(super::LOGIN_ENV_SENTINEL_END.as_bytes());

        let parsed = super::parse_login_shell_environment(&stdout);

        assert_eq!(parsed.get("GOOD").map(String::as_str), Some("yes"));
        assert_eq!(parsed.get("ALSO_GOOD").map(String::as_str), Some("yes"));
        assert!(!parsed.contains_key("BAD"));
    }

    /// The user's own editor choice reaches a pane the same way whether the app
    /// was launched from a terminal or from the Dock. The PTY layer still
    /// installs its own editor when the profile sets neither.
    #[cfg(unix)]
    #[test]
    fn login_env_parse_imports_the_editor_the_profile_chose() {
        let stdout = format!(
            "{}EDITOR=hx\0VISUAL=hx\0{}",
            super::LOGIN_ENV_SENTINEL,
            super::LOGIN_ENV_SENTINEL_END
        );

        let parsed = super::parse_login_shell_environment(stdout.as_bytes());

        assert_eq!(parsed.get("EDITOR").map(String::as_str), Some("hx"));
        assert_eq!(parsed.get("VISUAL").map(String::as_str), Some("hx"));
    }

    /// NUL delimiting is what makes a multi-line export survive the round trip.
    #[cfg(unix)]
    #[test]
    fn login_env_parse_keeps_values_containing_newlines_and_equals_signs() {
        let stdout = format!(
            "{}NODE_EXTRA_CA_CERTS=-----BEGIN-----\nline2\n-----END-----\0CONN=a=b=c\0{}",
            super::LOGIN_ENV_SENTINEL,
            super::LOGIN_ENV_SENTINEL_END
        );

        let parsed = super::parse_login_shell_environment(stdout.as_bytes());

        assert_eq!(
            parsed.get("NODE_EXTRA_CA_CERTS").map(String::as_str),
            Some("-----BEGIN-----\nline2\n-----END-----")
        );
        assert_eq!(parsed.get("CONN").map(String::as_str), Some("a=b=c"));
    }

    /// Keys the PTY layer owns are dropped at the source, so a profile can
    /// never rename the terminal or hand a pane the capture shell's own working
    /// directory. `PATH` is excluded because it is already unioned separately.
    /// Pane identity is not defended here — `configure_pty_environment` scrubs
    /// `OPTIONAL_PTY_ENV` after the fill and owns that invariant.
    #[cfg(unix)]
    #[test]
    fn login_env_parse_drops_keys_the_pty_layer_owns() {
        let stdout = format!(
            "{}PWD=/tmp/capture\0ZDOTDIR=/tmp/z\0TERM=xterm-kitty\0PATH=/only/profile\0KEEP=yes\0{}",
            super::LOGIN_ENV_SENTINEL,
            super::LOGIN_ENV_SENTINEL_END
        );

        let parsed = super::parse_login_shell_environment(stdout.as_bytes());

        for owned in ["PWD", "ZDOTDIR", "TERM", "PATH"] {
            assert!(
                !parsed.contains_key(owned),
                "{owned} should not be imported"
            );
        }
        assert_eq!(parsed.get("KEEP").map(String::as_str), Some("yes"));
    }

    #[cfg(unix)]
    #[test]
    fn a_shell_without_a_locale_gets_a_utf8_one() {
        let mut environment = HashMap::from([("LANG".to_string(), String::new())]);
        ensure_utf8_locale(&mut environment);
        assert_eq!(environment.get("LANG").map(String::as_str), Some("C.UTF-8"));
    }

    #[cfg(unix)]
    #[test]
    fn the_shell_locale_is_kept() {
        let mut environment = HashMap::from([("LC_CTYPE".to_string(), "fr_FR.UTF-8".to_string())]);
        ensure_utf8_locale(&mut environment);
        assert!(!environment.contains_key("LANG"));
        assert_eq!(
            environment.get("LC_CTYPE").map(String::as_str),
            Some("fr_FR.UTF-8")
        );
    }

    /// A shell that fails, or output with no sentinel at all, must degrade to
    /// "no profile environment" rather than to garbage entries.
    #[cfg(unix)]
    #[test]
    fn login_env_parse_yields_nothing_without_a_sentinel() {
        assert!(super::parse_login_shell_environment(b"HOME=/Users/x\0").is_empty());
        assert!(super::parse_login_shell_environment(b"").is_empty());
        // Opening fence but no closing one: the shell died mid-capture, so the
        // payload is unterminated and cannot be told apart from later output.
        let truncated = format!("{}HOME=/Users/x\0", super::LOGIN_ENV_SENTINEL);
        assert!(super::parse_login_shell_environment(truncated.as_bytes()).is_empty());
    }
}
