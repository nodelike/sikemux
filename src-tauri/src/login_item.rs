//! While remote access is on, the background core starts when the person logs
//! in, so a paired phone reaches this Mac after a restart without Sikemux
//! being opened. A LaunchAgent in `~/Library/LaunchAgents` runs the same
//! command the app starts the core with; turning remote access off removes it.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// How the app starts the core, which the LaunchAgent repeats.
pub(crate) struct CoreLaunch {
    pub binary: PathBuf,
    pub socket: PathBuf,
    pub log: PathBuf,
    pub args: Vec<OsString>,
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn string(value: impl AsRef<std::ffi::OsStr>) -> String {
    format!(
        "<string>{}</string>",
        escape(&value.as_ref().to_string_lossy())
    )
}

/// The LaunchAgent: started at login, and again only if it crashed, so Quit
/// and Stop Everything or a second core finding one running stays stopped.
pub(crate) fn agent_plist(label: &str, launch: &CoreLaunch, path_env: Option<&str>) -> String {
    let mut arguments = vec![string(&launch.binary), string("core"), string("--socket")];
    arguments.push(string(&launch.socket));
    arguments.extend(launch.args.iter().map(string));
    let environment = path_env
        .map(|path| {
            format!(
                "  <key>EnvironmentVariables</key>\n  <dict>\n    <key>PATH</key>\n    {}\n  </dict>\n",
                string(path)
            )
        })
        .unwrap_or_default();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  {label}
  <key>ProgramArguments</key>
  <array>
    {arguments}
  </array>
{environment}  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>
  <key>LimitLoadToSessionType</key>
  <string>Aqua</string>
  <key>StandardOutPath</key>
  {log}
  <key>StandardErrorPath</key>
  {log}
</dict>
</plist>
"#,
        label = string(label),
        arguments = arguments.join("\n    "),
        log = string(&launch.log),
    )
}

pub(crate) fn label(identifier: &str) -> String {
    format!("{identifier}.core")
}

fn agent_path(label: &str) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").filter(|home| !home.is_empty())?;
    Some(
        PathBuf::from(home)
            .join("Library/LaunchAgents")
            .join(format!("{label}.plist")),
    )
}

fn write_if_changed(path: &Path, contents: &str) -> std::io::Result<()> {
    if std::fs::read_to_string(path).is_ok_and(|current| current == contents) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, contents)
}

/// Installs or removes the LaunchAgent to match `enabled`. Installing takes
/// effect at the next login; the core already runs for this one.
pub(crate) fn sync(identifier: &str, launch: Option<&CoreLaunch>, enabled: bool) {
    if !cfg!(target_os = "macos") {
        return;
    }
    let label = label(identifier);
    let Some(path) = agent_path(&label) else {
        return;
    };
    if enabled {
        let Some(launch) = launch else {
            return;
        };
        let path_env = std::env::var("PATH").ok();
        let plist = agent_plist(&label, launch, path_env.as_deref());
        if let Err(error) = write_if_changed(&path, &plist) {
            eprintln!("Sikemux could not start its core at login: {error}");
        }
        return;
    }
    if !path.exists() {
        return;
    }
    let _ = sikemux_process::user_environment::command("/bin/launchctl")
        .arg("bootout")
        .arg(format!("gui/{}/{label}", uid()))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    if let Err(error) = std::fs::remove_file(&path) {
        eprintln!("Sikemux could not stop starting its core at login: {error}");
    }
}

fn uid() -> u32 {
    // SAFETY: getuid takes no arguments, cannot fail and touches no memory.
    unsafe { libc::getuid() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launch() -> CoreLaunch {
        CoreLaunch {
            binary: "/Applications/Sikemux.app/Contents/MacOS/sikemux-editor".into(),
            socket: "/Users/me/.config/sikemux/core.sock".into(),
            log: "/Users/me/Library/Logs/com.nodelike.sikemux/core.log".into(),
            args: vec![
                "--data-dir".into(),
                "/Users/me/Library/Application Support/A&B".into(),
            ],
        }
    }

    #[test]
    fn the_agent_runs_the_core_the_way_the_app_does() {
        let plist = agent_plist(
            "com.nodelike.sikemux.core",
            &launch(),
            Some("/usr/bin:/bin"),
        );
        assert!(plist.contains("<string>com.nodelike.sikemux.core</string>"));
        assert!(plist.contains(
            "<string>/Applications/Sikemux.app/Contents/MacOS/sikemux-editor</string>\n    <string>core</string>\n    <string>--socket</string>\n    <string>/Users/me/.config/sikemux/core.sock</string>\n    <string>--data-dir</string>"
        ));
        assert!(plist.contains("Application Support/A&amp;B"));
        assert!(plist.contains("<key>PATH</key>\n    <string>/usr/bin:/bin</string>"));
        assert!(plist.contains("<key>SuccessfulExit</key>\n    <false/>"));
    }

    #[test]
    fn the_label_follows_the_app_identifier() {
        assert_eq!(
            label("com.nodelike.sikemux.dev"),
            "com.nodelike.sikemux.dev.core"
        );
    }

    #[test]
    fn an_unchanged_agent_is_not_rewritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Library/LaunchAgents/x.plist");
        write_if_changed(&path, "one").unwrap();
        let written = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        write_if_changed(&path, "one").unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            written
        );
        write_if_changed(&path, "two").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "two");
    }
}
