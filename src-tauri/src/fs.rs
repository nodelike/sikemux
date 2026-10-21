use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use base64::{engine::general_purpose, Engine as _};
use serde_json::Value;
use tauri::async_runtime::spawn_blocking;

use crate::error::{AppError, AppResult};

/// The file tree's and the editor's methods on this host's own disk: the same
/// `files.*` the core serves other devices, run here so nothing waits on the
/// core's connection, and a whole file read at once.
#[tauri::command]
pub async fn files_call(
    manager: tauri::State<'_, crate::pty::PtyManager>,
    method: String,
    params: Value,
) -> AppResult<Value> {
    use sikemux_core::files;
    let path = params
        .get("path")
        .and_then(Value::as_str)
        .map(PathBuf::from);
    let (method, answer) = spawn_blocking(move || {
        let answer = files::run(&method, params, files::MAX_TEXT_BYTES);
        (method, answer)
    })
    .await
    .map_err(|e| AppError::Other(format!("files_call join: {e}")))?;
    let answer = answer.map_err(|error| crate::pty::core_message(error.to_string()))?;
    if let Some(path) = path.filter(|_| files::saved(&method, &answer)) {
        person_wrote(&manager, path);
    }
    Ok(answer)
}

/// Tells the core the person wrote `path`, so the change is theirs even
/// while an agent is at work in the same checkout.
fn person_wrote(manager: &crate::pty::PtyManager, path: PathBuf) {
    let Some(client) = manager.current_client() else {
        return;
    };
    tauri::async_runtime::spawn(async move {
        let params = serde_json::json!({ "path": path });
        let _ = client.call("personSaved", params).await;
    });
}

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

const INLINE_TEXT_MAX_BYTES: u64 = 1024 * 1024;

/// Refuses anything but a regular file. Opening without blocking means a named
/// pipe is turned away instead of hanging until something writes to it.
pub(crate) fn read_bounded(path: &Path, max_bytes: u64) -> AppResult<Vec<u8>> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(AppError::Fs(format!(
            "{} is not a regular file",
            path.display()
        )));
    }
    let mut bytes = Vec::with_capacity(metadata.len().min(max_bytes) as usize);
    file.take(max_bytes + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max_bytes {
        return Err(AppError::Fs(format!(
            "{} exceeds the {} read limit",
            path.display(),
            human_bytes(max_bytes)
        )));
    }
    Ok(bytes)
}

#[tauri::command]
pub async fn read_text_file_limited(path: String) -> AppResult<String> {
    spawn_blocking(move || read_text_file_limited_sync(path))
        .await
        .map_err(|e| AppError::Other(format!("read_text_file_limited join: {e}")))?
}

fn read_text_file_limited_sync(path: String) -> AppResult<String> {
    let meta = fs::metadata(&path)?;
    if meta.len() > INLINE_TEXT_MAX_BYTES {
        return Err(AppError::Fs(format!(
            "{} is too large for inline diff ({})",
            path,
            human_bytes(meta.len())
        )));
    }
    let bytes = read_bounded(Path::new(&path), INLINE_TEXT_MAX_BYTES)?;
    if bytes.iter().take(8192).any(|b| *b == 0) {
        return Err(AppError::Fs(format!(
            "{} is binary; inline diff is disabled",
            path
        )));
    }
    String::from_utf8(bytes).map_err(|_| {
        AppError::Fs(format!(
            "{} is not UTF-8 text; inline diff is disabled",
            path
        ))
    })
}

fn human_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

#[tauri::command]
pub async fn open_in_default_app(path: String) -> AppResult<()> {
    spawn_blocking(move || open::that_detached(&path).map_err(AppError::from))
        .await
        .map_err(|e| AppError::Other(format!("open_in_default_app join: {e}")))?
}

/// Copy an external file into a target directory, preserving its basename.
/// If a same-named file already exists we append " (N)" until unique, so
/// Finder-drop never overwrites silently. Returns the final landing path.
#[tauri::command]
pub async fn copy_into_dir(src: String, dir: String) -> AppResult<String> {
    spawn_blocking(move || copy_into_dir_sync(src, dir))
        .await
        .map_err(|e| AppError::Other(format!("copy_into_dir join: {e}")))?
}

fn copy_into_dir_sync(src: String, dir: String) -> AppResult<String> {
    let src_path = std::path::PathBuf::from(&src);
    if !src_path.exists() {
        return Err(AppError::Fs(format!("source missing: {}", src)));
    }
    let name = src_path
        .file_name()
        .ok_or_else(|| AppError::Fs(format!("source has no filename: {}", src)))?
        .to_os_string();
    write_new_in_dir(Path::new(&dir), &name, |destination| {
        let mut source = fs::File::open(&src_path)?;
        std::io::copy(&mut source, destination)?;
        Ok(())
    })
}

/// Write a new file into `dir` under `name`, or under "name (N)" when that is
/// taken, so nothing already there is ever overwritten. Returns where it landed,
/// and leaves nothing behind if the writing itself fails.
fn write_new_in_dir(
    dir: &Path,
    name: &std::ffi::OsStr,
    fill: impl FnOnce(&mut fs::File) -> std::io::Result<()>,
) -> AppResult<String> {
    fs::create_dir_all(dir)?;
    let stem = Path::new(name)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = Path::new(name)
        .extension()
        .map(|s| format!(".{}", s.to_string_lossy()))
        .unwrap_or_default();

    let mut fill = Some(fill);
    for n in 0..1000 {
        let candidate = if n == 0 {
            dir.join(name)
        } else {
            dir.join(format!("{stem} ({n}){ext}"))
        };
        let mut destination = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(AppError::from(error)),
        };
        let written = fill.take().expect("a name is only opened once")(&mut destination)
            .and_then(|()| destination.sync_all());
        if let Err(error) = written {
            drop(destination);
            let _ = fs::remove_file(&candidate);
            return Err(AppError::from(error));
        }
        return Ok(candidate.to_string_lossy().into_owned());
    }

    Err(AppError::Fs(format!(
        "no available destination name for {} after 1000 attempts",
        name.to_string_lossy()
    )))
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
        write_new_in_dir(&dir, &file_name, |destination| destination.write_all(&png)).map(Some)
    })
    .await
    .map_err(|e| AppError::Other(format!("save_clipboard_image join: {e}")))?
}

/// Write base64 bytes into `dir` under `name`, the way a copied file lands
/// there: never over something already named that. Returns the final path.
#[tauri::command]
pub async fn save_base64_into_dir(dir: String, name: String, data: String) -> AppResult<String> {
    spawn_blocking(move || save_base64_into_dir_sync(dir, name, data))
        .await
        .map_err(|e| AppError::Other(format!("save_base64_into_dir join: {e}")))?
}

// The name may be a path an agent wrote. Only its last part can say where the file goes.
fn leaf_name(name: &str) -> AppResult<std::ffi::OsString> {
    Path::new(name)
        .file_name()
        .filter(|name| !name.is_empty())
        .map(|name| name.to_os_string())
        .ok_or(AppError::BadArg("a saved file needs a name"))
}

fn save_base64_into_dir_sync(dir: String, name: String, data: String) -> AppResult<String> {
    let file_name = leaf_name(&name)?;
    let bytes = general_purpose::STANDARD
        .decode(data.as_bytes())
        .map_err(|_| AppError::BadArg("a saved file needs base64 contents"))?;
    write_new_in_dir(Path::new(&dir), &file_name, |destination| {
        destination.write_all(&bytes)
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn reads_refuse_pipes_and_devices_without_blocking() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pipe = dir.path().join("pipe");
        let name = std::ffi::CString::new(pipe.to_string_lossy().as_bytes()).unwrap();
        // SAFETY: `name` is a nul-terminated path that outlives the call.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(matches!(read_bounded(&pipe, 1024), Err(AppError::Fs(_))));
        assert!(matches!(
            read_bounded(Path::new("/dev/zero"), 1024),
            Err(AppError::Fs(_))
        ));
        assert!(read_bounded(dir.path(), 1024).is_err());

        let file = dir.path().join("big");
        fs::write(&file, vec![b'a'; 2048]).unwrap();
        assert!(matches!(read_bounded(&file, 1024), Err(AppError::Fs(_))));
        assert_eq!(read_bounded(&file, 4096).unwrap().len(), 2048);
    }

    #[test]
    fn a_saved_picture_never_lands_on_one_already_there() {
        let dir = tempfile::tempdir().expect("tempdir");
        let folder = dir.path().to_string_lossy().into_owned();
        let bytes = general_purpose::STANDARD.encode(b"a picture");

        let first = save_base64_into_dir_sync(folder.clone(), "shot.png".into(), bytes.clone())
            .expect("the first save must land");
        let second = save_base64_into_dir_sync(folder.clone(), "shot.png".into(), bytes.clone())
            .expect("the second save must land beside it");
        assert_eq!(first, dir.path().join("shot.png").to_string_lossy());
        assert_eq!(second, dir.path().join("shot (1).png").to_string_lossy());
        assert_eq!(fs::read(&first).expect("read back"), b"a picture");
        assert_eq!(fs::read(&second).expect("read back"), b"a picture");
    }

    #[test]
    fn a_saved_picture_cannot_be_named_its_way_out_of_the_folder() {
        let dir = tempfile::tempdir().expect("tempdir");
        let folder = dir.path().to_string_lossy().into_owned();
        let bytes = general_purpose::STANDARD.encode(b"a picture");

        let landed =
            save_base64_into_dir_sync(folder.clone(), "../escaped.png".into(), bytes.clone())
                .expect("the save must land");
        assert_eq!(landed, dir.path().join("escaped.png").to_string_lossy());

        assert!(
            matches!(
                save_base64_into_dir_sync(folder, "shot.png".into(), "not base64!".into()),
                Err(AppError::BadArg(_))
            ),
            "contents that are not base64 must be refused"
        );
    }
}
