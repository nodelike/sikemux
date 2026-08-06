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
    if let Some(cwd) = cwd.filter(|value| !value.is_empty()) {
        process.current_dir(cwd);
    }
    for (key, value) in env {
        if key.starts_with("SIKEMUX_") && key.bytes().all(|b| b.is_ascii_uppercase() || b == b'_') {
            process.env(key, value);
        }
    }
    let output = tokio::time::timeout(std::time::Duration::from_secs(120), process.output())
        .await
        .map_err(|_| AppError::Other("background command timed out after 120 seconds".into()))?
        .map_err(AppError::Io)?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    if text.len() > 16_000 {
        text.truncate(16_000);
        text.push_str("\n… output truncated");
    }
    Ok(BackgroundCommandResult {
        code: output.status.code().unwrap_or(-1),
        output: text,
    })
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
