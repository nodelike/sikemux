// Browser-routing helpers used by the cloud (AWS / future GCP) sign-in flow.
//
//  * open_url      — open a URL, optionally in a specific browser app, with
//                    an optional workspace-switch keystroke first.
//  * macos_focus_app — JUST the workspace switch: activate <app> and fire
//                    the configured shortcut. No URL open. Used before
//                    `aws sso login` so the CLI's device-auth URL lands on
//                    the right space (and we avoid opening a second tab).
//
// On macOS the workspace-switch uses `tell process <app>` + `key code N`
// (US-QWERTY hardware key codes) so the keystroke is delivered to the right
// process regardless of focus races + keyboard layout.

#[cfg(target_os = "macos")]
use std::process::Command;

use crate::error::{AppError, AppResult};
use std::collections::HashMap;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};

const BACKGROUND_TIMEOUT: Duration = Duration::from_secs(120);
const BACKGROUND_KILL_GRACE: Duration = Duration::from_millis(250);
const BACKGROUND_STREAM_LIMIT: usize = 8_000;
const BACKGROUND_OUTPUT_LIMIT: usize = BACKGROUND_STREAM_LIMIT * 2;
const TRUNCATED_SUFFIX: &str = "\n… output truncated";

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundCommandResult {
    code: i32,
    output: String,
}

#[tauri::command]
pub async fn run_background_command(
    command: String,
    cwd: Option<String>,
    env: HashMap<String, String>,
) -> AppResult<BackgroundCommandResult> {
    if command.trim().is_empty() || command.len() > 8_000 {
        return Err(AppError::BadArg(
            "background command must be 1..8000 characters",
        ));
    }
    let mut process = if cfg!(windows) {
        let mut child = tokio::process::Command::new("powershell.exe");
        child.args(["-NoLogo", "-NonInteractive", "-Command", &command]);
        child
    } else {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        let mut child = tokio::process::Command::new(shell);
        child.args(["-lc", &command]);
        child
    };
    // A dropped tokio child does not otherwise guarantee termination. This
    // is the final safety net if the task itself is cancelled (window close,
    // app shutdown, runtime teardown) before the explicit timeout path runs.
    process.kill_on_drop(true);
    process.stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(unix)]
    process.process_group(0);
    if let Some(cwd) = cwd.filter(|value| !value.is_empty()) {
        process.current_dir(cwd);
    }
    for (key, value) in env {
        if key.starts_with("SIKEMUX_") && key.bytes().all(|b| b.is_ascii_uppercase() || b == b'_') {
            process.env(key, value);
        }
    }
    let mut child = process.spawn().map_err(AppError::Io)?;
    let pid = child.id();
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| AppError::Other("background command stdout unavailable".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| AppError::Other("background command stderr unavailable".into()))?;

    // Drain both pipes concurrently so a noisy stderr cannot deadlock a
    // command whose stdout is also full. Each collector retains a fixed
    // prefix and discards the remainder while continuing to drain.
    let completion = async {
        tokio::try_join!(
            child.wait(),
            read_bounded(stdout, BACKGROUND_STREAM_LIMIT),
            read_bounded(stderr, BACKGROUND_STREAM_LIMIT),
        )
    };
    let (status, stdout, stderr) = match tokio::time::timeout(BACKGROUND_TIMEOUT, completion).await
    {
        Ok(result) => result.map_err(AppError::Io)?,
        Err(_) => {
            terminate_background_process(&mut child, pid).await;
            return Err(AppError::Other(
                "background command timed out after 120 seconds".into(),
            ));
        }
    };
    let truncated = stdout.truncated || stderr.truncated;
    let mut bytes = stdout.bytes;
    bytes.extend(stderr.bytes);
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if truncated || text.len() > BACKGROUND_OUTPUT_LIMIT {
        truncate_utf8(
            &mut text,
            BACKGROUND_OUTPUT_LIMIT.saturating_sub(TRUNCATED_SUFFIX.len()),
        );
        text.push_str(TRUNCATED_SUFFIX);
    }
    Ok(BackgroundCommandResult {
        code: status.code().unwrap_or(-1),
        output: text,
    })
}

struct BoundedRead {
    bytes: Vec<u8>,
    truncated: bool,
}

async fn read_bounded<R>(mut reader: R, limit: usize) -> std::io::Result<BoundedRead>
where
    R: AsyncRead + Unpin,
{
    let mut retained = Vec::with_capacity(limit);
    let mut buf = [0u8; 8 * 1024];
    let mut truncated = false;
    loop {
        let read = reader.read(&mut buf).await?;
        if read == 0 {
            break;
        }
        let remaining = limit.saturating_sub(retained.len());
        let keep = remaining.min(read);
        retained.extend_from_slice(&buf[..keep]);
        truncated |= keep < read;
    }
    Ok(BoundedRead {
        bytes: retained,
        truncated,
    })
}

fn truncate_utf8(text: &mut String, max_bytes: usize) {
    if text.len() <= max_bytes {
        return;
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
}

async fn terminate_background_process(child: &mut tokio::process::Child, pid: Option<u32>) {
    #[cfg(unix)]
    {
        if let Some(pid) = pid {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGTERM);
            }
        }
        // The direct shell may already have exited while a descendant still
        // owns a pipe. Do not use child.wait() as a tree-liveness proxy:
        // always give the process group its grace window, then force-kill any
        // holdout and reap the direct child.
        tokio::time::sleep(BACKGROUND_KILL_GRACE).await;
        if let Some(pid) = pid {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
        let _ = child.kill().await;
        let _ = child.wait().await;
    }

    #[cfg(windows)]
    {
        if let Some(pid) = pid {
            let _ = tokio::process::Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/T"])
                .status()
                .await;
        }
        tokio::time::sleep(BACKGROUND_KILL_GRACE).await;
        if let Some(pid) = pid {
            let _ = tokio::process::Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .status()
                .await;
        }
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
}

fn validate_external_url(raw: &str) -> AppResult<()> {
    let url = url::Url::parse(raw).map_err(|_| AppError::BadArg("invalid external URL"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(AppError::BadArg("only HTTP(S) URLs may be opened"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(AppError::BadArg(
            "credentials in external URLs are not allowed",
        ));
    }
    Ok(())
}

#[tauri::command]
pub async fn open_url(url: String, app: Option<String>, shortcut: Option<String>) -> AppResult<()> {
    validate_external_url(&url)?;
    let app = app.filter(|s| !s.trim().is_empty());
    let shortcut = shortcut.filter(|s| !s.trim().is_empty());
    tauri::async_runtime::spawn_blocking(move || {
        #[cfg(target_os = "macos")]
        {
            if let Some(app_name) = app.as_deref() {
                run_focus(app_name, shortcut.as_deref());
                Command::new("open")
                    .arg("-a")
                    .arg(app_name)
                    .arg(&url)
                    .status()?;
                return Ok(());
            }
            Command::new("open").arg(&url).status()?;
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (app, shortcut);
            open::that(&url)?;
        }
        Ok(())
    })
    .await
    .map_err(|e| AppError::Other(format!("open_url join: {e}")))?
}

#[tauri::command]
pub async fn macos_focus_app(app: String, shortcut: Option<String>) -> AppResult<()> {
    if app.trim().is_empty() {
        return Ok(()); // no-op when no app configured
    }
    #[cfg(target_os = "macos")]
    {
        let shortcut = shortcut.filter(|s| !s.trim().is_empty());
        tauri::async_runtime::spawn_blocking(move || run_focus(&app, shortcut.as_deref()))
            .await
            .map_err(|e| AppError::Other(format!("macos_focus_app join: {e}")))?;
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, shortcut);
    Ok(())
}

// ---- macOS osascript path ----------------------------------------------

#[cfg(target_os = "macos")]
fn run_focus(app_name: &str, shortcut: Option<&str>) {
    let script = build_activate_and_switch(app_name, shortcut);
    let _ = Command::new("osascript").arg("-e").arg(&script).status();
}

#[cfg(target_os = "macos")]
fn build_activate_and_switch(app_name: &str, shortcut: Option<&str>) -> String {
    let app_esc = escape_applescript(app_name);
    let mut s = format!("tell application \"{}\" to activate\ndelay 0.4\n", app_esc);
    if let Some(sc) = shortcut.and_then(parse_shortcut) {
        let (mods, key) = sc;
        s.push_str("tell application \"System Events\"\n");
        s.push_str(&format!("  tell process \"{}\"\n", app_esc));
        if let Some(code) = key_code_for(&key) {
            s.push_str(&format!("    key code {} using {{{}}}\n", code, mods));
        } else {
            s.push_str(&format!(
                "    keystroke \"{}\" using {{{}}}\n",
                escape_applescript(&key),
                mods
            ));
        }
        s.push_str("  end tell\n");
        s.push_str("end tell\n");
        s.push_str("delay 0.2");
    }
    s
}

#[cfg(target_os = "macos")]
fn escape_applescript(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Parse "ctrl+3" / "cmd+shift+1" into AppleScript modifier list + bare key.
#[cfg(target_os = "macos")]
fn parse_shortcut(input: &str) -> Option<(String, String)> {
    let mut parts: Vec<&str> = input.split('+').map(str::trim).collect();
    if parts.len() < 2 {
        return None;
    }
    let key = parts.pop()?.to_string();
    if key.is_empty() {
        return None;
    }
    let mut mods: Vec<&str> = Vec::new();
    for p in parts {
        match p.to_lowercase().as_str() {
            "ctrl" | "control" => mods.push("control down"),
            "cmd" | "command" | "meta" => mods.push("command down"),
            "alt" | "opt" | "option" => mods.push("option down"),
            "shift" => mods.push("shift down"),
            _ => return None,
        }
    }
    Some((mods.join(", "), key))
}

/// US-QWERTY virtual key codes — layout-independent so synthesized events
/// reach apps (like Firefox-based browsers) that read `keyCode` not `key`.
#[cfg(target_os = "macos")]
fn key_code_for(key: &str) -> Option<u8> {
    let k = key.to_ascii_lowercase();
    if k.len() != 1 {
        return None;
    }
    Some(match k.as_str() {
        "1" => 18,
        "2" => 19,
        "3" => 20,
        "4" => 21,
        "5" => 23,
        "6" => 22,
        "7" => 26,
        "8" => 28,
        "9" => 25,
        "0" => 29,
        "a" => 0,
        "b" => 11,
        "c" => 8,
        "d" => 2,
        "e" => 14,
        "f" => 3,
        "g" => 5,
        "h" => 4,
        "i" => 34,
        "j" => 38,
        "k" => 40,
        "l" => 37,
        "m" => 46,
        "n" => 45,
        "o" => 31,
        "p" => 35,
        "q" => 12,
        "r" => 15,
        "s" => 1,
        "t" => 17,
        "u" => 32,
        "v" => 9,
        "w" => 13,
        "x" => 7,
        "y" => 16,
        "z" => 6,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::{read_bounded, truncate_utf8};
    use tokio::io::AsyncWriteExt;

    #[test]
    fn utf8_truncation_stops_on_a_character_boundary() {
        let mut text = "abc🦀def".to_string();
        truncate_utf8(&mut text, 5);
        assert_eq!(text, "abc");
        assert!(text.is_char_boundary(text.len()));
    }

    #[tokio::test]
    async fn bounded_reader_drains_but_retains_only_the_limit() {
        let (mut writer, reader) = tokio::io::duplex(64);
        let producer = tokio::spawn(async move {
            writer.write_all(&vec![b'x'; 4_096]).await.unwrap();
        });
        let output = read_bounded(reader, 128).await.unwrap();
        producer.await.unwrap();
        assert_eq!(output.bytes.len(), 128);
        assert!(output.truncated);
    }
}
