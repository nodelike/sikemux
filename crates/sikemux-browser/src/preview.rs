//! A page an agent drew, loaded in a view of its own off screen so the agent can check
//! it without a tab opening on its desk. The view goes away with the `Preview`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

use crate::host::Host;
use crate::{
    viewport, DocumentEvent, Opener, PageDialog, PageLoad, Tab, TabEvents, TabSpec, TabStall,
    USER_AGENT,
};

static NEXT: AtomicU64 = AtomicU64::new(1);

pub(crate) struct Preview {
    view: Tab,
    load: watch::Receiver<Option<Result<(), String>>>,
}

impl Preview {
    pub(crate) fn open(
        host: &dyn Host,
        agent_id: &str,
        url: &str,
        size: (f64, f64),
    ) -> Result<Self, String> {
        let parent = host.window()?;
        let tab_id = format!("page-preview-{}", NEXT.fetch_add(1, Ordering::Relaxed));
        let (sender, load) = watch::channel(None);
        let view = Tab::open(
            parent,
            TabSpec {
                agent_id,
                tab_id: &tab_id,
                url,
                user_agent: USER_AGENT,
                size,
                opener: None,
            },
            Arc::new(PreviewEvents { load: sender }),
        )?;
        crate::manager::place(&view, viewport::Layout::Parked { page: size }, true);
        view.show();
        Ok(Self { view, load })
    }

    pub(crate) fn view(&self) -> &Tab {
        &self.view
    }

    pub(crate) async fn loaded(&mut self, timeout: Duration) -> Result<(), String> {
        match tokio::time::timeout(timeout, self.load.wait_for(Option::is_some)).await {
            Ok(Ok(load)) => load.clone().unwrap_or(Ok(())),
            Ok(Err(_)) => Err("the page went away while loading".into()),
            Err(_) => Err("the page took too long to load".into()),
        }
    }
}

impl Drop for Preview {
    fn drop(&mut self) {
        self.view.close();
    }
}

struct PreviewEvents {
    load: watch::Sender<Option<Result<(), String>>>,
}

impl PreviewEvents {
    /// A failed load may still report its end, which must not hide the failure.
    fn finished(&self) {
        self.load
            .send_if_modified(|load| load.is_none() && load.replace(Ok(())).is_none());
    }
}

impl TabEvents for PreviewEvents {
    fn may_load(&self, url: &str) -> bool {
        url::Url::parse(url).is_ok_and(|url| crate::manager::tab_may_load(&url))
    }

    fn loaded(&self, load: PageLoad) {
        if !load.started {
            self.finished();
        }
    }

    fn titled(&self, _title: String) {}

    fn moved(&self, _url: String, _can_go_back: bool, _can_go_forward: bool) {}

    fn new_window(&self, _url: String, _opener: Option<Opener>) -> Option<Tab> {
        None
    }

    fn closed(&self) {}

    fn download_started(&self, _url: String, _suggested: &Path) -> Option<PathBuf> {
        None
    }

    fn download_finished(&self, _url: String, _saved: bool) {}

    fn dialog(&self, _dialog: Option<PageDialog>) {}

    fn upload(&self) -> Option<Vec<PathBuf>> {
        None
    }

    fn document(&self, event: DocumentEvent) {
        match event {
            DocumentEvent::Finished { .. } => self.finished(),
            DocumentEvent::Failed { error, .. } => {
                self.load
                    .send_replace(Some(Err(format!("the page failed to load: {error}"))));
            }
            DocumentEvent::Started { .. } | DocumentEvent::Responded { .. } => {}
        }
    }

    fn health(&self, _stall: Option<TabStall>) {}
}
