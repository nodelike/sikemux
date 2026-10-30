//! Tells the core when the network comes back, changes, or the computer wakes, so
//! everything that waits on the network can try again at once instead of
//! sitting out its backoff.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use tokio::sync::mpsc;

use super::{remote, Core};
use crate::protocol::{Event, NetworkChange as NetworkEvent};

/// Passes each change on to whatever waits on the network, then tells the app.
pub(super) async fn run(core: Arc<Core>) {
    let core = Arc::downgrade(&core);
    let mut events = watch();
    while let Some(change) = events.recv().await {
        let Some(core) = core.upgrade() else {
            return;
        };
        if change != NetworkEvent::Offline {
            remote::network_back(&core).await;
            core.chats.retry_offline_failures();
        }
        core.broadcast_local(&Event::Network { change });
    }
}

/// What the system says about the route out: whether there is one, and over which interfaces.
#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetworkPath {
    pub online: bool,
    pub interfaces: Vec<String>,
}

/// What a new path means after the last one. The first path only sets the scene.
#[cfg(any(target_os = "macos", test))]
pub fn classify(previous: Option<&NetworkPath>, next: &NetworkPath) -> Option<NetworkEvent> {
    let previous = previous?;
    match (previous.online, next.online) {
        (false, true) => Some(NetworkEvent::Online),
        (true, false) => Some(NetworkEvent::Offline),
        (true, true) if previous.interfaces != next.interfaces => Some(NetworkEvent::Changed),
        _ => None,
    }
}

const WAKE_CHECK: Duration = Duration::from_secs(5);
/// More than this between the wall clock and the clock that stops in sleep means the computer slept.
const SLEPT: Duration = Duration::from_secs(20);

/// Whether the computer slept between two readings. The monotonic clock stands
/// still during sleep while the wall clock does not.
fn slept(since: (Instant, SystemTime), now: (Instant, SystemTime)) -> bool {
    let awake = now.0.duration_since(since.0);
    let wall = now.1.duration_since(since.1).unwrap_or_default();
    wall > awake + SLEPT
}

/// Starts watching for the life of the process.
pub fn watch() -> mpsc::UnboundedReceiver<NetworkEvent> {
    let (events, received) = mpsc::unbounded_channel();
    let wakes = events.clone();
    tokio::spawn(async move {
        let mut last = (Instant::now(), SystemTime::now());
        loop {
            tokio::time::sleep(WAKE_CHECK).await;
            let now = (Instant::now(), SystemTime::now());
            if slept(last, now) && wakes.send(NetworkEvent::Woke).is_err() {
                return;
            }
            last = now;
        }
    });
    paths::watch(events);
    received
}

#[cfg(target_os = "macos")]
mod paths {
    use std::ffi::{c_char, c_void, CStr};
    use std::ptr::NonNull;
    use std::sync::{Arc, Mutex};

    use block2::RcBlock;
    use sikemux_process::sync::Locked;
    use tokio::sync::mpsc;

    use super::{classify, NetworkEvent, NetworkPath};

    type Object = *mut c_void;

    const SATISFIED: i32 = 1;

    #[link(name = "Network", kind = "framework")]
    extern "C" {
        fn nw_path_monitor_create() -> Object;
        fn nw_path_monitor_set_queue(monitor: Object, queue: Object);
        fn nw_path_monitor_set_update_handler(
            monitor: Object,
            handler: &block2::Block<dyn Fn(Object)>,
        );
        fn nw_path_monitor_start(monitor: Object);
        fn nw_path_get_status(path: Object) -> i32;
        fn nw_path_enumerate_interfaces(path: Object, each: &block2::Block<dyn Fn(Object) -> u8>);
        fn nw_interface_get_name(interface: Object) -> *const c_char;
    }

    extern "C" {
        fn dispatch_queue_create(label: *const c_char, attributes: Object) -> Object;
    }

    fn read(path: Object) -> NetworkPath {
        let names = Arc::new(Mutex::new(Vec::new()));
        let found = names.clone();
        let each = RcBlock::new(move |interface: Object| -> u8 {
            // SAFETY: the interface is valid for the call, and its name lives as long as it does.
            let name = unsafe { nw_interface_get_name(interface) };
            if let Some(name) = NonNull::new(name.cast_mut()) {
                // SAFETY: Network.framework hands back a NUL-terminated C string.
                let name = unsafe { CStr::from_ptr(name.as_ptr()) };
                found.locked().push(name.to_string_lossy().into_owned());
            }
            1
        });
        // SAFETY: the path is valid for the length of the update handler that passed it.
        let online = unsafe {
            nw_path_enumerate_interfaces(path, &each);
            nw_path_get_status(path) == SATISFIED
        };
        let mut interfaces = names.locked().clone();
        interfaces.sort();
        interfaces.dedup();
        NetworkPath { online, interfaces }
    }

    pub(super) fn watch(events: mpsc::UnboundedSender<NetworkEvent>) {
        let last: Mutex<Option<NetworkPath>> = Mutex::new(None);
        let handler = RcBlock::new(move |path: Object| {
            let next = read(path);
            let mut last = last.locked();
            if let Some(event) = classify(last.as_ref(), &next) {
                let _ = events.send(event);
            }
            *last = Some(next);
        });
        // SAFETY: the monitor and its queue live for the rest of the process and
        // are never released, so the handler they hold stays valid too.
        unsafe {
            let monitor = nw_path_monitor_create();
            if monitor.is_null() {
                return;
            }
            let queue = dispatch_queue_create(c"sikemux.network".as_ptr(), std::ptr::null_mut());
            nw_path_monitor_set_queue(monitor, queue);
            nw_path_monitor_set_update_handler(monitor, &handler);
            nw_path_monitor_start(monitor);
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod paths {
    use tokio::sync::mpsc;

    use super::NetworkEvent;

    /// Other systems tell the core only about waking, until they get a watcher of their own.
    pub(super) fn watch(_events: mpsc::UnboundedSender<NetworkEvent>) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(online: bool, interfaces: &[&str]) -> NetworkPath {
        NetworkPath {
            online,
            interfaces: interfaces.iter().map(|name| (*name).to_owned()).collect(),
        }
    }

    #[test]
    fn a_path_says_online_offline_or_changed_against_the_last() {
        let wifi = path(true, &["en0"]);
        assert_eq!(classify(None, &wifi), None);
        assert_eq!(
            classify(Some(&path(false, &[])), &wifi),
            Some(NetworkEvent::Online)
        );
        assert_eq!(
            classify(Some(&wifi), &path(false, &[])),
            Some(NetworkEvent::Offline)
        );
        assert_eq!(
            classify(Some(&wifi), &path(true, &["en7"])),
            Some(NetworkEvent::Changed)
        );
        assert_eq!(classify(Some(&wifi), &wifi), None);
        assert_eq!(
            classify(Some(&path(false, &[])), &path(false, &["en0"])),
            None
        );
    }

    #[test]
    fn sleep_shows_as_the_wall_clock_running_ahead() {
        let start = (Instant::now(), SystemTime::now());
        let awake = (
            start.0 + Duration::from_secs(5),
            start.1 + Duration::from_secs(5),
        );
        let after_sleep = (
            start.0 + Duration::from_secs(5),
            start.1 + Duration::from_secs(600),
        );
        assert!(!slept(start, awake));
        assert!(slept(start, after_sleep));
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn the_system_monitor_runs_and_the_watch_stays_open() {
        let mut events = watch();
        let quiet = tokio::time::timeout(Duration::from_millis(500), events.recv()).await;
        assert!(quiet.is_err(), "a steady network says nothing: {quiet:?}");
    }
}
