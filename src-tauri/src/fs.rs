use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use tauri::async_runtime::spawn_blocking;

use crate::error::{AppError, AppResult};

/// A screenshot dragged straight from the macOS thumbnail lives in a folder
/// only the app it was dropped on may open, and only until the thumbnail goes.
fn is_screenshot_thumbnail(path: &str) -> bool {
    path.contains("/TemporaryItems/NSIRD_screencaptureui_")
}

/// Copy dropped screenshot thumbnails somewhere an agent can read them, and
/// answer every path with the one to hand on.
#[tauri::command]
pub async fn keep_dropped_paths(paths: Vec<String>) -> AppResult<Vec<String>> {
    spawn_blocking(move || {
        let dir = std::env::temp_dir().join("sikemux-drops");
        paths
            .into_iter()
            .map(|path| {
                if !is_screenshot_thumbnail(&path) {
                    return path;
                }
                keep_copy(Path::new(&path), &dir).unwrap_or(path)
            })
            .collect()
    })
    .await
    .map_err(|e| AppError::Other(format!("keep_dropped_paths join: {e}")))
}

fn keep_copy(source: &Path, dir: &Path) -> Option<String> {
    let name = source.file_name()?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis();
    let target_dir = dir.join(stamp.to_string());
    fs::create_dir_all(&target_dir).ok()?;
    let target = target_dir.join(name);
    fs::copy(source, &target).ok()?;
    Some(target.to_string_lossy().into_owned())
}

#[tauri::command]
pub async fn open_in_default_app(path: String) -> AppResult<()> {
    spawn_blocking(move || open::that_detached(&path).map_err(AppError::from))
        .await
        .map_err(|e| AppError::Other(format!("open_in_default_app join: {e}")))?
}

/// Where the platform puts downloads. Falls back to the temp directory, which
/// is what the browser pane's own downloads do.
#[tauri::command]
pub async fn downloads_dir(app: tauri::AppHandle) -> AppResult<String> {
    use tauri::Manager;
    let dir = app
        .path()
        .download_dir()
        .unwrap_or_else(|_| std::env::temp_dir());
    Ok(dir.to_string_lossy().into_owned())
}

#[cfg(target_os = "macos")]
fn clipboard_png() -> Option<Vec<u8>> {
    use objc2_app_kit::{
        NSBitmapImageFileType, NSBitmapImageRep, NSPasteboard, NSPasteboardTypePNG,
        NSPasteboardTypeTIFF,
    };
    use objc2_foundation::NSDictionary;

    let pasteboard = NSPasteboard::generalPasteboard();
    // SAFETY: objc2 does not tie NSPasteboard to the main thread, and the type is an
    // immutable AppKit constant.
    if let Some(png) = unsafe { pasteboard.dataForType(NSPasteboardTypePNG) } {
        return Some(png.to_vec());
    }
    // Screenshots land on the clipboard as TIFF.
    // SAFETY: as above.
    let tiff = unsafe { pasteboard.dataForType(NSPasteboardTypeTIFF) }?;
    let bitmap = NSBitmapImageRep::imageRepWithData(&tiff)?;
    let empty = NSDictionary::new();
    // SAFETY: NSBitmapImageRep works off the main thread, and an empty properties
    // dictionary is allowed.
    let png =
        unsafe { bitmap.representationUsingType_properties(NSBitmapImageFileType::PNG, &empty) }?;
    Some(png.to_vec())
}

#[cfg(not(target_os = "macos"))]
fn clipboard_png() -> Option<Vec<u8>> {
    None
}

fn chat_attachment_path(app: &tauri::AppHandle) -> PathBuf {
    use tauri::Manager;
    app.path()
        .app_cache_dir()
        .unwrap_or_else(|_| std::env::temp_dir())
        .join("pasted")
}

/// Pictures pasted into a chat are kept in the app cache, not the project.
#[tauri::command]
pub async fn chat_attachment_dir(app: tauri::AppHandle) -> AppResult<String> {
    let dir = chat_attachment_path(&app);
    std::fs::create_dir_all(&dir)?;
    Ok(dir.to_string_lossy().into_owned())
}

/// Saves the clipboard's picture beside other pasted ones; `None` when it holds no picture.
#[tauri::command]
pub async fn save_clipboard_image(
    app: tauri::AppHandle,
    name: String,
) -> AppResult<Option<String>> {
    let dir = chat_attachment_path(&app);
    spawn_blocking(move || {
        let Some(png) = clipboard_png() else {
            return Ok(None);
        };
        let file_name = leaf_name(&name)?;
        sikemux_core::files::write_new_in_dir(&dir, &file_name, |destination| {
            destination.write_all(&png)
        })
        .map(Some)
        .map_err(|error| AppError::Fs(error.to_string()))
    })
    .await
    .map_err(|e| AppError::Other(format!("save_clipboard_image join: {e}")))?
}

// The name may be a path an agent wrote. Only its last part can say where the file goes.
fn leaf_name(name: &str) -> AppResult<std::ffi::OsString> {
    Path::new(name)
        .file_name()
        .filter(|name| !name.is_empty())
        .map(|name| name.to_os_string())
        .ok_or(AppError::BadArg("a saved file needs a name"))
}

/// Reveal a path in the OS file manager, selecting the entry itself.
/// macOS: `open -R` highlights the file inside its folder in Finder. Other
/// platforms open the containing directory (no portable "select" exists).
#[tauri::command]
pub async fn reveal_in_finder(path: String) -> AppResult<()> {
    spawn_blocking(move || reveal_in_finder_sync(path))
        .await
        .map_err(|e| AppError::Other(format!("reveal_in_finder join: {e}")))?
}

fn reveal_in_finder_sync(path: String) -> AppResult<()> {
    let p = std::path::PathBuf::from(&path);
    if !p.exists() {
        return Err(AppError::Fs(format!("path missing: {}", path)));
    }
    #[cfg(target_os = "macos")]
    {
        sikemux_process::user_environment::command("/usr/bin/open")
            .arg("-R")
            .arg(&p)
            .status()?;
    }
    #[cfg(target_os = "windows")]
    {
        sikemux_process::user_environment::command("explorer.exe")
            .arg("/select,")
            .arg(&p)
            .status()?;
    }
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        let dir = if p.is_dir() {
            p.clone()
        } else {
            p.parent()
                .map(|d| d.to_path_buf())
                .unwrap_or_else(|| p.clone())
        };
        sikemux_process::user_environment::command("xdg-open")
            .arg(&dir)
            .status()?;
    }
    Ok(())
}
