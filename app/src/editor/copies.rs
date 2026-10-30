//! Read-only copies of other hosts' pictures and documents, so the viewer shows them
//! from this disk as it shows this host's files. They live in a folder of this app's
//! own under the temporary folder, which keeps under a size and is removed on quit.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use base64::Engine;
use gpui_kit::AppContext;
use serde_json::json;

use super::{Editors, FileKey};
use crate::core::Target;

/// The most the copies take together; the oldest go first to make room.
const CAP: u64 = 512 * 1024 * 1024;
/// How much of a file each request brings, the most the core answers with.
const PIECE: u64 = 1024 * 1024;
const FOLDER: &str = "sikemux-copies-";

#[derive(Default)]
pub(super) struct Copies {
    /// Each copy with its size, oldest first.
    kept: Vec<(FileKey, PathBuf, u64)>,
    made: u64,
    cleared_on_quit: bool,
}

impl Copies {
    fn keep(&mut self, key: FileKey, path: PathBuf, size: u64) {
        self.forget(&key);
        self.kept.push((key, path, size));
        let mut total: u64 = self.kept.iter().map(|(_, _, size)| size).sum();
        while total > CAP && self.kept.len() > 1 {
            let (_, oldest, size) = self.kept.remove(0);
            remove(&oldest);
            total -= size;
        }
    }

    pub(super) fn forget(&mut self, key: &FileKey) {
        self.kept.retain(|(kept, path, _)| {
            let keep = kept != key;
            if !keep {
                remove(path);
            }
            keep
        });
    }
}

fn root() -> PathBuf {
    std::env::temp_dir().join(format!("{FOLDER}{}", std::process::id()))
}

/// Each copy sits in a folder of its own under its file's name, so its own app knows
/// the file by it, and a fresh copy never takes a path a picture was drawn from.
fn remove(path: &Path) {
    if let Some(folder) = path.parent() {
        let _ = fs::remove_dir_all(folder);
    }
}

/// Removes the copies of runs of this app that ended without quitting.
fn clear_abandoned() {
    let Ok(entries) = fs::read_dir(std::env::temp_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .and_then(|name| name.strip_prefix(FOLDER))
            .and_then(|pid| pid.parse::<libc::pid_t>().ok())
        else {
            continue;
        };
        let running = unsafe { libc::kill(pid, 0) } == 0
            || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
        if !running {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

/// Brings the file of `key`, `size` bytes long, over from its host into a fresh copy,
/// and answers where the copy is once it is kept.
pub(super) async fn bring(
    editors: gpui_kit::WeakEntity<Editors>,
    target: Target,
    key: FileKey,
    size: u64,
    cx: &mut gpui_kit::AsyncApp,
) -> Result<PathBuf, String> {
    if size > CAP {
        return Err("This file is too large to bring over from its host.".into());
    }
    let made = editors
        .update(cx, |editors, cx| {
            let copies = &mut editors.copies;
            if !copies.cleared_on_quit {
                copies.cleared_on_quit = true;
                cx.background_spawn(async { clear_abandoned() }).detach();
                cx.on_app_quit(|_, _| {
                    let _ = fs::remove_dir_all(root());
                    async {}
                })
                .detach();
            }
            copies.made += 1;
            copies.made
        })
        .map_err(|_| "The window closed.".to_owned())?;
    let name = key
        .path
        .file_name()
        .map_or_else(|| "file".into(), |name| name.to_owned());
    let path = root().join(made.to_string()).join(name);
    let written = fetch(&target, &key, &path, cx).await;
    if let Err(error) = written {
        remove(&path);
        return Err(error);
    }
    let kept = path.clone();
    editors
        .update(cx, |editors, _| editors.copies.keep(key, kept, size))
        .map_err(|_| "The window closed.".to_owned())?;
    Ok(path)
}

async fn fetch(
    target: &Target,
    key: &FileKey,
    path: &Path,
    cx: &mut gpui_kit::AsyncApp,
) -> Result<(), String> {
    let folder = path.parent().map(Path::to_owned).unwrap_or_default();
    let created = path.to_owned();
    let mut file = cx
        .background_spawn(async move {
            fs::create_dir_all(&folder)?;
            fs::File::create(&created)
        })
        .await
        .map_err(|error| error.to_string())?;
    let name = key.path.to_string_lossy().into_owned();
    let mut at = 0;
    loop {
        let read = json!({ "path": name, "at": at, "length": PIECE });
        let piece = target.request("files.bytes", read).await?;
        let size = piece["size"].as_u64().unwrap_or(0);
        let encoded = piece["base64"].as_str().unwrap_or_default().to_owned();
        let (written, returned) = cx
            .background_spawn(async move {
                let written = base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .map_err(|error| error.to_string())
                    .and_then(|bytes| {
                        file.write_all(&bytes)
                            .map(|()| bytes.len() as u64)
                            .map_err(|error| error.to_string())
                    });
                (written, file)
            })
            .await;
        file = returned;
        let written = written?;
        at += written;
        if written == 0 || at >= size {
            break;
        }
        if at > CAP {
            return Err("This file is too large to bring over from its host.".into());
        }
    }
    let mut readable = file
        .metadata()
        .map_err(|error| error.to_string())?
        .permissions();
    readable.set_readonly(true);
    fs::set_permissions(path, readable).map_err(|error| error.to_string())
}
