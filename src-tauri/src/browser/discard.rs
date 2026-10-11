//! When macOS runs short of memory, tabs nobody has looked at for a while let
//! go of their page, as Safari's do. The tab stays in the strip with its title,
//! icon and address, and loads that address again once it is shown or an agent
//! works in it.

use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};

use super::{BrowserManager, TabStrip};

/// How long a tab must have been out of sight before it may be unloaded.
const WARNING_HIDDEN: Duration = Duration::from_secs(30 * 60);
const CRITICAL_HIDDEN: Duration = Duration::from_secs(5 * 60);
const PROBE_TIMEOUT: Duration = Duration::from_millis(1500);

/// True while the page plays sound or holds something typed into a form.
const BUSY_SCRIPT: &str = r#"
const audible = [...document.querySelectorAll("video, audio")].some(
    (media) => !media.paused && !media.ended && !media.muted && media.volume > 0,
);
const focused = document.activeElement;
const typing = !!focused && (focused.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(focused.tagName));
const edited = [...document.querySelectorAll("input, textarea")].some((field) =>
    field.type === "checkbox" || field.type === "radio"
        ? field.checked !== field.defaultChecked
        : field.type !== "hidden" && field.value !== field.defaultValue,
);
return String(audible || typing || edited);
"#;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pressure {
    Warning,
    Critical,
}

impl Pressure {
    fn hidden_long_enough(self) -> Duration {
        match self {
            Pressure::Warning => WARNING_HIDDEN,
            Pressure::Critical => CRITICAL_HIDDEN,
        }
    }

    /// Reads the bits a memory pressure dispatch source reports.
    pub fn from_flags(flags: usize) -> Option<Pressure> {
        const WARN: usize = 0x2;
        const CRITICAL: usize = 0x4;
        if flags & CRITICAL != 0 {
            Some(Pressure::Critical)
        } else if flags & WARN != 0 {
            Some(Pressure::Warning)
        } else {
            None
        }
    }
}

impl TabStrip {
    /// A tab may go once it has been out of sight and idle long enough. A tab
    /// on screen, loading, or used by an agent a moment ago has no hidden time.
    pub fn may_unload(&self, id: &str, now: Instant, pressure: Pressure) -> bool {
        !self.unloaded.contains(id)
            && !self.awake(id)
            && self.hidden_since.get(id).is_some_and(|since| {
                now.saturating_duration_since(*since) >= pressure.hidden_long_enough()
            })
    }

    pub fn to_unload(&self, now: Instant, pressure: Pressure) -> Vec<String> {
        self.order
            .iter()
            .filter(|id| self.may_unload(id, now, pressure))
            .cloned()
            .collect()
    }
}

impl BrowserManager {
    pub async fn relieve(&self, app: &AppHandle, pressure: Pressure) {
        if self
            .relieving
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            return;
        }
        let now = Instant::now();
        for (agent_id, tab_id, view) in self.unload_candidates(now, pressure) {
            let busy =
                tokio::time::timeout(PROBE_TIMEOUT, super::tools::run_helper(&view, BUSY_SCRIPT))
                    .await;
            let keep = match busy {
                Ok(Ok(answer)) => answer != "false",
                _ => pressure == Pressure::Warning,
            };
            if !keep {
                self.unload(app, &agent_id, &tab_id, now, pressure);
            }
        }
        self.relieving
            .store(false, std::sync::atomic::Ordering::Release);
    }

    fn unload_candidates(
        &self,
        now: Instant,
        pressure: Pressure,
    ) -> Vec<(String, String, tauri::Webview)> {
        let agents = self.lock();
        agents
            .iter()
            .filter(|(agent_id, _)| !self.recording(agent_id))
            .flat_map(|(agent_id, agent)| {
                agent
                    .strip
                    .to_unload(now, pressure)
                    .into_iter()
                    .filter(|tab_id| !self.held(tab_id))
                    .filter_map(|tab_id| {
                        let view = agent.views.get(&tab_id)?.clone();
                        Some((agent_id.clone(), tab_id, view))
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// A page with a dialog open, files waiting for its chooser, or a download
    /// under way would lose them along with the page.
    fn held(&self, tab_id: &str) -> bool {
        self.dialogs_lock().contains_key(tab_id)
            || self.uploads_lock().contains_key(tab_id)
            || self.downloads_lock().keys().any(|(tab, _)| tab == tab_id)
    }

    fn recording(&self, agent_id: &str) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.recordings_lock().contains_key(agent_id)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = agent_id;
            false
        }
    }

    fn unload(
        &self,
        app: &AppHandle,
        agent_id: &str,
        tab_id: &str,
        now: Instant,
        pressure: Pressure,
    ) {
        let view = {
            let mut agents = self.lock();
            let Some(agent) = agents.get_mut(agent_id) else {
                return;
            };
            if !agent.strip.may_unload(tab_id, now, pressure) {
                return;
            }
            let Some(view) = agent.views.remove(tab_id) else {
                return;
            };
            agent.applied.remove(tab_id);
            agent.strip.unloaded.insert(tab_id.to_owned());
            if let Some(page) = agent.strip.pages.get_mut(tab_id) {
                page.can_go_back = false;
                page.can_go_forward = false;
            }
            view
        };
        self.stalls_lock().remove(tab_id);
        super::drop_view(view);
        self.announce(app);
    }
}

/// Listens for the system running short of memory, for as long as the app runs.
#[cfg(target_os = "macos")]
pub fn watch_pressure(app: AppHandle) {
    use block2::{Block, RcBlock};
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[repr(C)]
    struct SourceType {
        _private: [u8; 0],
    }
    extern "C" {
        static _dispatch_source_type_memorypressure: SourceType;
        fn dispatch_source_create(
            kind: *const SourceType,
            handle: usize,
            mask: usize,
            queue: *mut c_void,
        ) -> *mut c_void;
        fn dispatch_source_set_event_handler(source: *mut c_void, handler: &Block<dyn Fn()>);
        fn dispatch_source_get_data(source: *mut c_void) -> usize;
        fn dispatch_resume(object: *mut c_void);
    }
    const WARN: usize = 0x2;
    const CRITICAL: usize = 0x4;
    static SOURCE: AtomicUsize = AtomicUsize::new(0);

    // SAFETY: the memory pressure type takes no handle and a mask of its own flags; a
    // null queue means the default global queue.
    let source = unsafe {
        dispatch_source_create(
            &raw const _dispatch_source_type_memorypressure,
            0,
            WARN | CRITICAL,
            std::ptr::null_mut(),
        )
    };
    if source.is_null() || SOURCE.swap(source as usize, Ordering::AcqRel) != 0 {
        return;
    }
    let handler: RcBlock<dyn Fn()> = RcBlock::new(move || {
        let source = SOURCE.load(Ordering::Acquire) as *mut c_void;
        // SAFETY: the source is never released, so it outlives every event it delivers.
        let flags = unsafe { dispatch_source_get_data(source) };
        let Some(pressure) = Pressure::from_flags(flags) else {
            return;
        };
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let manager = app.state::<BrowserManager>();
            manager.relieve(&app, pressure).await;
        });
    });
    // SAFETY: `source` is live and suspended until resumed; dispatch copies the block.
    unsafe {
        dispatch_source_set_event_handler(source, &handler);
        dispatch_resume(source);
    }
}

#[cfg(not(target_os = "macos"))]
pub fn watch_pressure(_: AppHandle) {}

#[cfg(test)]
mod tests {
    use super::super::viewport::Layout;
    use super::super::{AgentBrowser, BrowserBounds, TabPage};
    use super::*;

    const MINUTE: Duration = Duration::from_secs(60);

    fn strip(tabs: &[&str]) -> TabStrip {
        let mut strip = TabStrip::default();
        for id in tabs {
            strip.insert((*id).into(), TabPage::default());
        }
        strip
    }

    #[test]
    fn only_tabs_hidden_long_enough_for_the_pressure_go() {
        let start = Instant::now();
        let mut strip = strip(&["old", "recent", "fresh"]);
        strip.hidden_since.insert("old".into(), start);
        strip
            .hidden_since
            .insert("recent".into(), start + 25 * MINUTE);
        strip
            .hidden_since
            .insert("fresh".into(), start + 39 * MINUTE);
        let now = start + 40 * MINUTE;

        assert_eq!(strip.to_unload(now, Pressure::Warning), vec!["old"]);
        assert_eq!(
            strip.to_unload(now, Pressure::Critical),
            vec!["old", "recent"]
        );
    }

    #[test]
    fn loading_acting_settling_or_already_unloaded_tabs_stay() {
        let start = Instant::now();
        let mut strip = strip(&["loading", "acting", "settling", "gone", "idle"]);
        for id in strip.order.clone() {
            strip.hidden_since.insert(id, start);
        }
        strip.pages.get_mut("loading").unwrap().loading = true;
        strip.acting.insert("acting".into(), 1);
        strip.settle("settling", 2);
        strip.unloaded.insert("gone".into());

        assert_eq!(
            strip.to_unload(start + 120 * MINUTE, Pressure::Critical),
            vec!["idle"]
        );
    }

    #[test]
    fn an_unloaded_tab_comes_back_once_shown_or_acted_in_and_stays_parked_until_loaded() {
        let mut agent = AgentBrowser {
            strip: strip(&["a", "b"]),
            ..AgentBrowser::default()
        };
        agent
            .strip
            .unloaded
            .extend(["a".to_owned(), "b".to_owned()]);
        assert!(agent.needed_back().is_empty());

        agent.bounds = Some(BrowserBounds {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
            clip_left: 0.0,
            clip_right: 0.0,
            holes: Vec::new(),
            dim: 0.0,
            opacity: 1.0,
        });
        assert_eq!(agent.needed_back(), vec!["b"]);
        agent.strip.acting.insert("a".into(), 1);
        let mut needed = agent.needed_back();
        needed.sort();
        assert_eq!(needed, vec!["a", "b"]);

        agent.strip.unloaded.clear();
        agent.strip.revealing.insert("b".into(), 2);
        assert!(matches!(agent.layout_of("b"), Layout::Parked { .. }));
        assert!(agent.strip.release_revealing("b", 2));
        assert!(matches!(agent.layout_of("b"), Layout::Shown { .. }));
    }

    #[test]
    fn reads_the_pressure_the_system_reports() {
        assert_eq!(Pressure::from_flags(0x1), None);
        assert_eq!(Pressure::from_flags(0x2), Some(Pressure::Warning));
        assert_eq!(Pressure::from_flags(0x4), Some(Pressure::Critical));
        assert_eq!(Pressure::from_flags(0x6), Some(Pressure::Critical));
    }
}
