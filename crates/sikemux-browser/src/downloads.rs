use std::collections::VecDeque;

use serde::Serialize;

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
}
