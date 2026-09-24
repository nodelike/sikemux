use std::path::{Path, PathBuf};

use base64::Engine;
use serde::Serialize;
use tauri::AppHandle;

use crate::error::{AppError, AppResult};

const MAX_IMAGE_BYTES: u64 = 40 * 1024 * 1024;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Wallpaper {
    name: String,
    data_url: String,
}

#[tauri::command]
pub async fn wallpaper_image(app: AppHandle) -> AppResult<Wallpaper> {
    let path = wallpaper_path(&app).await?;
    tauri::async_runtime::spawn_blocking(move || read_wallpaper(&path))
        .await
        .map_err(|error| AppError::Other(format!("wallpaper_image join: {error}")))?
}

fn read_wallpaper(path: &Path) -> AppResult<Wallpaper> {
    let image = aerial_thumbnail(path).unwrap_or_else(|| path.to_path_buf());
    let path = image.as_path();
    if !path.is_file() {
        return Err(AppError::Other(format!(
            "the wallpaper at {} is not an image file",
            path.display()
        )));
    }
    if path.metadata()?.len() > MAX_IMAGE_BYTES {
        return Err(AppError::Other("the wallpaper is too large to read".into()));
    }
    let (bytes, mime) = image_bytes(path)?;
    let name = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Wallpaper".into());
    Ok(Wallpaper {
        name,
        data_url: format!(
            "data:{mime};base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        ),
    })
}

/// macOS wallpapers are often HEIC, which the web view may not decode, so `sips` shrinks them to a PNG first.
#[cfg(target_os = "macos")]
fn image_bytes(path: &Path) -> AppResult<(Vec<u8>, &'static str)> {
    let out = tempfile::Builder::new().suffix(".png").tempfile()?;
    let status = std::process::Command::new("/usr/bin/sips")
        .args(["-s", "format", "png", "-Z", "320"])
        .arg(path)
        .arg("--out")
        .arg(out.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    if !status.success() {
        return Err(AppError::Other(format!(
            "could not read the wallpaper at {}",
            path.display()
        )));
    }
    Ok((std::fs::read(out.path())?, "image/png"))
}

/// An aerial wallpaper is a `.madesktop` plist; its still frame lives at `thumbnailPath`.
#[cfg(target_os = "macos")]
fn aerial_thumbnail(path: &Path) -> Option<PathBuf> {
    if path.extension()? != "madesktop" {
        return None;
    }
    let output = std::process::Command::new("/usr/bin/plutil")
        .args(["-extract", "thumbnailPath", "raw", "-o", "-"])
        .arg(path)
        .output()
        .ok()?;
    let thumbnail = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    thumbnail.is_file().then_some(thumbnail)
}

#[cfg(not(target_os = "macos"))]
fn aerial_thumbnail(_path: &Path) -> Option<PathBuf> {
    None
}

#[cfg(not(target_os = "macos"))]
fn image_bytes(path: &Path) -> AppResult<(Vec<u8>, &'static str)> {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let mime = match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        _ => {
            return Err(AppError::Other(format!(
                "unsupported wallpaper format: .{extension}"
            )))
        }
    };
    Ok((std::fs::read(path)?, mime))
}

#[cfg(target_os = "macos")]
async fn wallpaper_path(app: &AppHandle) -> AppResult<PathBuf> {
    let (send, receive) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        let _ = send.send(main_screen_wallpaper());
    })
    .map_err(|error| AppError::Other(format!("wallpaper: {error}")))?;
    receive
        .await
        .map_err(|_| AppError::Other("wallpaper: the main thread dropped the request".into()))?
        .ok_or_else(|| AppError::Other("macOS did not report a wallpaper for this screen".into()))
}

#[cfg(target_os = "macos")]
fn main_screen_wallpaper() -> Option<PathBuf> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSScreen, NSWorkspace};
    let screen = NSScreen::mainScreen(MainThreadMarker::new()?)?;
    let url = NSWorkspace::sharedWorkspace().desktopImageURLForScreen(&screen)?;
    Some(PathBuf::from(url.path()?.to_string()))
}

#[cfg(target_os = "linux")]
async fn wallpaper_path(_app: &AppHandle) -> AppResult<PathBuf> {
    let omarchy = crate::system::user_home().join(".config/omarchy/current/background");
    if let Ok(resolved) = omarchy.canonicalize() {
        return Ok(resolved);
    }
    for key in ["picture-uri-dark", "picture-uri"] {
        let Ok(output) = std::process::Command::new("gsettings")
            .args(["get", "org.gnome.desktop.background", key])
            .output()
        else {
            continue;
        };
        let raw = String::from_utf8_lossy(&output.stdout);
        let uri = raw.trim().trim_matches('\'');
        if let Ok(url) = url::Url::parse(uri) {
            if let Ok(path) = url.to_file_path() {
                if path.is_file() {
                    return Ok(path);
                }
            }
        }
    }
    Err(AppError::Other(
        "could not find the desktop wallpaper".into(),
    ))
}

#[cfg(target_os = "windows")]
async fn wallpaper_path(_app: &AppHandle) -> AppResult<PathBuf> {
    let output = std::process::Command::new("reg")
        .args(["query", r"HKCU\Control Panel\Desktop", "/v", "WallPaper"])
        .output()?;
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| {
            line.split_once("REG_SZ")
                .map(|(_, value)| PathBuf::from(value.trim()))
        })
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| AppError::Other("could not find the desktop wallpaper".into()))
}
