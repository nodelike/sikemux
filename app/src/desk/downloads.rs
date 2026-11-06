//! What a desk's pages download: the note it gets, and where the file is
//! kept.

use std::path::{Path, PathBuf};

use gpui_kit::component::button::Button;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::WindowExt;
use gpui_kit::ParentElement;
use sikemux_browser::{BrowserDownload, DownloadState};

use crate::{toast, ui};

/// What a page's download says: it started, failed, or finished with a way to keep the file.
pub(crate) fn downloaded(download: &BrowserDownload) -> Notification {
    let held = PathBuf::from(&download.path);
    let name = held.file_name().map_or_else(
        || download.url.clone(),
        |name| name.to_string_lossy().into_owned(),
    );
    match download.state {
        DownloadState::Started => toast::info(format!("Downloading {name}")),
        DownloadState::Failed => toast::error(format!("Download of {name} failed")).autohide(false),
        DownloadState::Finished => {
            toast::success(format!("Downloaded {name}")).action(move |_, _, cx| {
                let (note, held) = (cx.entity(), held.clone());
                Button::new("keep-download")
                    .label("Keep in Downloads")
                    .children(ui::spot(|| "keep-download".into()))
                    .on_click(move |_, window, cx| {
                        match keep_download(&held) {
                            Ok(kept) => cx.reveal_path(&kept),
                            Err(error) => window.push_notification(
                                toast::error(format!("The download was not kept: {error}")),
                                cx,
                            ),
                        }
                        note.update(cx, |note, cx| note.dismiss(window, cx));
                    })
            })
        }
    }
}

/// Moves a file a page saved from the browser's own folder into Downloads, beside what is
/// there. A link keeps the file as macOS marked it on download and never replaces another.
pub(crate) fn keep_download(held: &Path) -> Result<PathBuf, String> {
    let downloads = sikemux_process::paths::home()
        .ok_or("this account has no home folder")?
        .join("Downloads");
    let name = held
        .file_name()
        .ok_or("the download has no name")?
        .to_string_lossy()
        .into_owned();
    let (stem, extension) = match name.rfind('.') {
        Some(dot) if dot > 0 => name.split_at(dot),
        _ => (name.as_str(), ""),
    };
    for n in 1..=100 {
        let kept = match n {
            1 => downloads.join(&name),
            n => downloads.join(format!("{stem} ({n}){extension}")),
        };
        match std::fs::hard_link(held, &kept) {
            Ok(()) => {
                let _ = std::fs::remove_file(held);
                return Ok(kept);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.to_string()),
        }
    }
    Err(format!("Downloads already has 100 files named like {name}"))
}
