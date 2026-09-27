//! The pages a tab has loaded as whole documents, with the answer each got.
//! A page's own recorder only sees the calls its scripts make, and a load that
//! fails never gets a page to record anything in.

use std::collections::VecDeque;
use std::time::Instant;

use serde::Serialize;

const MAX_DOCUMENTS: usize = 20;

/// What WebKit reports as a tab's top-level load moves along. `navigation`
/// tells loads apart, since a load that was cut short by the next one can
/// report its end after the next one started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocumentEvent {
    Started {
        navigation: usize,
        url: String,
    },
    Responded {
        url: String,
        status: Option<u16>,
        mime_type: String,
    },
    Finished {
        navigation: usize,
    },
    Failed {
        navigation: usize,
        error: String,
    },
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DocumentLoad {
    pub url: String,
    pub status: Option<u16>,
    pub mime_type: Option<String>,
    pub error: Option<String>,
    pub duration_ms: Option<u64>,
    #[serde(skip)]
    navigation: Option<usize>,
    #[serde(skip)]
    started: Option<Instant>,
    #[serde(skip)]
    open: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DocumentLog {
    loads: VecDeque<DocumentLoad>,
}

impl DocumentLog {
    /// Returns true when the event failed the tab's latest load, which leaves
    /// the tab with nothing more coming.
    pub fn note(&mut self, event: DocumentEvent) -> bool {
        match event {
            DocumentEvent::Started { navigation, url } => {
                self.begin(url, Some(navigation), Some(Instant::now()));
                false
            }
            DocumentEvent::Responded {
                url,
                status,
                mime_type,
            } => {
                if !self.loads.back().is_some_and(|load| load.open) {
                    self.begin(url.clone(), None, None);
                }
                if let Some(latest) = self.loads.back_mut() {
                    latest.url = url;
                    latest.status = status;
                    latest.mime_type = Some(mime_type).filter(|mime| !mime.is_empty());
                }
                false
            }
            DocumentEvent::Finished { navigation } => {
                if let Some(load) = self.open_load(navigation) {
                    load.close(None);
                }
                false
            }
            DocumentEvent::Failed { navigation, error } => {
                let latest = self.loads.len().checked_sub(1);
                let Some(index) = self.open_index(navigation) else {
                    return false;
                };
                self.loads[index].close(Some(error));
                Some(index) == latest
            }
        }
    }

    /// Oldest first. `since_current` keeps only the load that produced the
    /// page on screen and any tried after it.
    pub fn loads(&self, since_current: bool) -> Vec<DocumentLoad> {
        let from = if since_current {
            self.loads
                .iter()
                .rposition(|load| load.status.is_some() && load.error.is_none())
                .unwrap_or(0)
        } else {
            0
        };
        self.loads.iter().skip(from).cloned().collect()
    }

    fn begin(&mut self, url: String, navigation: Option<usize>, started: Option<Instant>) {
        self.loads.push_back(DocumentLoad {
            url,
            status: None,
            mime_type: None,
            error: None,
            duration_ms: None,
            navigation,
            started,
            open: true,
        });
        if self.loads.len() > MAX_DOCUMENTS {
            self.loads.pop_front();
        }
    }

    /// A tab's first load starts before anyone is listening, so it is first
    /// heard of by its answer and carries no navigation to match on.
    fn open_index(&self, navigation: usize) -> Option<usize> {
        self.loads
            .iter()
            .rposition(|load| load.open && load.navigation == Some(navigation))
            .or_else(|| {
                self.loads
                    .iter()
                    .rposition(|load| load.open && load.navigation.is_none())
            })
    }

    fn open_load(&mut self, navigation: usize) -> Option<&mut DocumentLoad> {
        let index = self.open_index(navigation)?;
        self.loads.get_mut(index)
    }
}

impl DocumentLoad {
    fn close(&mut self, error: Option<String>) {
        self.error = error;
        self.duration_ms = self
            .started
            .map(|started| started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64);
        self.open = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(log: &mut DocumentLog, navigation: usize, url: &str, status: Option<u16>) {
        log.note(DocumentEvent::Started {
            navigation,
            url: url.into(),
        });
        log.note(DocumentEvent::Responded {
            url: url.into(),
            status,
            mime_type: "text/html".into(),
        });
        log.note(DocumentEvent::Finished { navigation });
    }

    #[test]
    fn a_load_keeps_the_address_it_ended_at_and_its_status() {
        let mut log = DocumentLog::default();
        log.note(DocumentEvent::Started {
            navigation: 1,
            url: "http://a/old".into(),
        });
        log.note(DocumentEvent::Responded {
            url: "http://a/new".into(),
            status: Some(404),
            mime_type: "text/html".into(),
        });
        log.note(DocumentEvent::Finished { navigation: 1 });
        let loads = log.loads(false);
        assert_eq!(loads.len(), 1);
        assert_eq!(loads[0].url, "http://a/new");
        assert_eq!(loads[0].status, Some(404));
        assert_eq!(loads[0].mime_type.as_deref(), Some("text/html"));
        assert!(loads[0].duration_ms.is_some());
        assert_eq!(loads[0].error, None);
    }

    #[test]
    fn a_load_that_never_reached_the_server_carries_the_error() {
        let mut log = DocumentLog::default();
        log.note(DocumentEvent::Started {
            navigation: 1,
            url: "http://nowhere/".into(),
        });
        assert!(log.note(DocumentEvent::Failed {
            navigation: 1,
            error: "Could not connect to the server.".into(),
        }));
        let loads = log.loads(false);
        assert_eq!(loads[0].status, None);
        assert_eq!(
            loads[0].error.as_deref(),
            Some("Could not connect to the server.")
        );
    }

    #[test]
    fn a_load_cut_short_after_the_next_began_does_not_touch_the_next() {
        let mut log = DocumentLog::default();
        log.note(DocumentEvent::Started {
            navigation: 1,
            url: "http://a/slow".into(),
        });
        log.note(DocumentEvent::Started {
            navigation: 2,
            url: "http://a/fast".into(),
        });
        assert!(!log.note(DocumentEvent::Failed {
            navigation: 1,
            error: "cancelled".into(),
        }));
        log.note(DocumentEvent::Responded {
            url: "http://a/fast".into(),
            status: Some(200),
            mime_type: "text/html".into(),
        });
        log.note(DocumentEvent::Finished { navigation: 2 });
        let loads = log.loads(false);
        assert_eq!(loads[0].error.as_deref(), Some("cancelled"));
        assert_eq!(loads[1].error, None);
        assert_eq!(loads[1].status, Some(200));
    }

    #[test]
    fn since_current_starts_at_the_page_on_screen() {
        let mut log = DocumentLog::default();
        load(&mut log, 1, "http://a/1", Some(200));
        load(&mut log, 2, "http://a/2", Some(500));
        log.note(DocumentEvent::Started {
            navigation: 3,
            url: "http://a/3".into(),
        });
        log.note(DocumentEvent::Failed {
            navigation: 3,
            error: "offline".into(),
        });
        let urls: Vec<String> = log.loads(true).into_iter().map(|load| load.url).collect();
        assert_eq!(urls, ["http://a/2", "http://a/3"]);
        assert_eq!(log.loads(false).len(), 3);
    }

    #[test]
    fn a_load_first_heard_of_by_its_answer_is_still_kept() {
        let mut log = DocumentLog::default();
        log.note(DocumentEvent::Responded {
            url: "http://a/".into(),
            status: Some(200),
            mime_type: "text/html".into(),
        });
        log.note(DocumentEvent::Finished { navigation: 9 });
        let loads = log.loads(true);
        assert_eq!(loads.len(), 1);
        assert_eq!(loads[0].status, Some(200));
        assert_eq!(loads[0].duration_ms, None);
        assert!(!loads[0].open);
    }

    #[test]
    fn only_the_latest_loads_are_kept() {
        let mut log = DocumentLog::default();
        for page in 0..MAX_DOCUMENTS + 5 {
            load(&mut log, page, &format!("http://a/{page}"), Some(200));
        }
        let loads = log.loads(false);
        assert_eq!(loads.len(), MAX_DOCUMENTS);
        assert_eq!(loads[0].url, "http://a/5");
    }
}
