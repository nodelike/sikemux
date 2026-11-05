use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use serde::Serialize;
use url::Url;

use super::{BrowserDownload, DownloadState};

const KEPT: usize = 20;

/// The files an agent's tabs saved lately, oldest first. Each is numbered so an
/// action can pick out only the ones that began while it ran.
#[derive(Default)]
pub struct DownloadLog {
    next: u64,
    files: VecDeque<SavedFile>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SavedFile {
    #[serde(skip)]
    number: u64,
    pub tab_id: String,
    pub url: String,
    pub path: String,
    pub state: DownloadState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
}

impl DownloadLog {
    /// The number the next download will get.
    pub fn mark(&self) -> u64 {
        self.next
    }

    pub fn note(&mut self, download: &BrowserDownload, bytes: Option<u64>) {
        let started = self.files.iter_mut().rev().find(|file| {
            file.state == DownloadState::Started
                && file.tab_id == download.tab_id
                && file.url == download.url
        });
        match (download.state, started) {
            (DownloadState::Started, _) | (_, None) => {
                self.files.push_back(SavedFile {
                    number: self.next,
                    tab_id: download.tab_id.clone(),
                    url: download.url.clone(),
                    path: download.path.clone(),
                    state: download.state,
                    bytes,
                });
                self.next += 1;
                if self.files.len() > KEPT {
                    self.files.pop_front();
                }
            }
            (state, Some(file)) => {
                file.state = state;
                file.path = download.path.clone();
                file.bytes = bytes;
            }
        }
    }

    pub fn since(&self, mark: u64) -> Vec<SavedFile> {
        self.files
            .iter()
            .filter(|file| file.number >= mark)
            .cloned()
            .collect()
    }
}

/// Marks a download as from the internet, as Safari does, so Gatekeeper
/// checks it before it first opens.
#[cfg(target_os = "macos")]
pub(crate) fn quarantine(path: &Path) {
    use std::os::unix::ffi::OsStrExt;

    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default();
    let value = format!("0083;{seconds:08x};Sikemux;");
    let (Ok(target), Ok(name)) = (
        std::ffi::CString::new(path.as_os_str().as_bytes()),
        std::ffi::CString::new("com.apple.quarantine"),
    ) else {
        return;
    };
    // SAFETY: `target` and `name` are nul-terminated and live through the call, and
    // `value.len()` is exactly the bytes `value` points at.
    unsafe {
        libc::setxattr(
            target.as_ptr(),
            name.as_ptr(),
            value.as_ptr().cast(),
            value.len(),
            0,
            0,
        );
    }
}

/// The name the page suggested, else the last path segment of the URL, with
/// anything that could leave the download folder stripped out.
pub(crate) fn download_file_name(url: &Url, suggested: &Path) -> String {
    let candidate = suggested
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .or_else(|| {
            url.path_segments()
                .and_then(|mut segments| segments.next_back().map(str::to_owned))
                .filter(|name| !name.is_empty())
        })
        .unwrap_or_default();
    let cleaned: String = candidate
        .chars()
        .map(|char| {
            if matches!(char, '/' | '\\' | ':') {
                '_'
            } else {
                char
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.').to_owned();
    if cleaned.is_empty() {
        "download".into()
    } else {
        cleaned.chars().take(200).collect()
    }
}

/// `name`, `name (2)`, `name (3)`... whichever does not exist yet, keeping
/// the extension at the end.
pub(crate) fn unique_download_path(folder: &Path, name: &str) -> PathBuf {
    let first = folder.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, format!(".{extension}")),
        _ => (name, String::new()),
    };
    (2..)
        .map(|n| folder.join(format!("{stem} ({n}){extension}")))
        .find(|path| !path.exists())
        .expect("some numbered name is free")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn download(url: &str, state: DownloadState) -> BrowserDownload {
        BrowserDownload {
            agent_id: "agent".into(),
            tab_id: "tab".into(),
            url: url.into(),
            path: format!("/Downloads/{url}"),
            state,
        }
    }

    #[test]
    fn a_finished_download_updates_the_one_that_started() {
        let mut log = DownloadLog::default();
        log.note(&download("a.cer", DownloadState::Started), None);
        log.note(&download("a.cer", DownloadState::Finished), Some(1486));
        let files = log.since(0);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].state, DownloadState::Finished);
        assert_eq!(files[0].bytes, Some(1486));
    }

    #[test]
    fn an_action_sees_only_the_downloads_that_began_after_its_mark() {
        let mut log = DownloadLog::default();
        log.note(&download("old.pdf", DownloadState::Started), None);
        let mark = log.mark();
        log.note(&download("old.pdf", DownloadState::Finished), Some(10));
        log.note(&download("new.pdf", DownloadState::Started), None);
        let files = log.since(mark);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].url, "new.pdf");
    }

    #[test]
    fn only_the_latest_downloads_are_kept() {
        let mut log = DownloadLog::default();
        for index in 0..KEPT + 5 {
            log.note(
                &download(&format!("{index}.zip"), DownloadState::Failed),
                None,
            );
        }
        let files = log.since(0);
        assert_eq!(files.len(), KEPT);
        assert_eq!(files[0].url, "5.zip");
    }

    #[test]
    fn download_names_come_from_the_page_then_the_url_and_stay_in_the_folder() {
        let url = Url::parse("https://a.test/files/report.pdf?x=1").unwrap();
        assert_eq!(
            download_file_name(&url, Path::new("Quarterly.pdf")),
            "Quarterly.pdf"
        );
        assert_eq!(download_file_name(&url, Path::new("")), "report.pdf");
        assert_eq!(
            download_file_name(&url, Path::new("../../etc/passwd")),
            "passwd"
        );
        assert_eq!(download_file_name(&url, Path::new(".hidden")), "hidden");
        let bare = Url::parse("https://a.test/").unwrap();
        assert_eq!(download_file_name(&bare, Path::new("")), "download");
    }

    #[test]
    fn a_taken_download_name_gets_a_number_before_its_extension() {
        let folder = tempfile::tempdir().unwrap();
        assert_eq!(
            unique_download_path(folder.path(), "a.pdf"),
            folder.path().join("a.pdf")
        );
        std::fs::write(folder.path().join("a.pdf"), b"x").unwrap();
        std::fs::write(folder.path().join("a (2).pdf"), b"x").unwrap();
        assert_eq!(
            unique_download_path(folder.path(), "a.pdf"),
            folder.path().join("a (3).pdf")
        );
        std::fs::write(folder.path().join("notes"), b"x").unwrap();
        assert_eq!(
            unique_download_path(folder.path(), "notes"),
            folder.path().join("notes (2)")
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_finished_download_is_quarantined() {
        use std::os::unix::ffi::OsStrExt;

        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("setup.dmg");
        std::fs::write(&path, b"x").unwrap();
        quarantine(&path);
        let target = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        let name = std::ffi::CString::new("com.apple.quarantine").unwrap();
        let mut value = [0u8; 64];
        // SAFETY: `value` is a writable buffer of `value.len()` bytes, and both names are
        // nul-terminated and live through the call.
        let read = unsafe {
            libc::getxattr(
                target.as_ptr(),
                name.as_ptr(),
                value.as_mut_ptr().cast(),
                value.len(),
                0,
                0,
            )
        };
        let value = String::from_utf8_lossy(&value[..usize::try_from(read).unwrap()]);
        assert!(value.starts_with("0083;"), "{value}");
        assert!(value.ends_with(";Sikemux;"), "{value}");
    }
}
