//! A Windows core started at sign-in comes back when it is killed, as launchd and systemd
//! bring theirs back, and stays down when it is stopped.

#![cfg(windows)]

use std::path::Path;
use std::time::{Duration, Instant};

use sikemux_core::client::{ensure_running, frozen_request, probe};
use sikemux_core::frozen::FrozenRequest;
use sikemux_hostkit::login_item::{self, CoreLaunch};

const IDENTIFIER: &str = "com.nodelike.sikemux.restart-test";
const COME_BACK: Duration = Duration::from_secs(30);
const STAYS_DOWN: Duration = Duration::from_secs(8);

fn answering(socket: &Path) -> Option<u32> {
    probe(socket, Duration::from_secs(1))
        .ok()
        .map(|hello| hello.pid)
}

fn back_after(socket: &Path, killed: u32) -> Option<u32> {
    let deadline = Instant::now() + COME_BACK;
    while Instant::now() < deadline {
        match answering(socket) {
            Some(pid) if pid != killed => return Some(pid),
            _ => std::thread::sleep(Duration::from_millis(200)),
        }
    }
    None
}

/// The core `stopped` may answer while it shuts down, but none after it.
fn stays_down(socket: &Path, stopped: u32) -> bool {
    let deadline = Instant::now() + STAYS_DOWN;
    while Instant::now() < deadline {
        if answering(socket).is_some_and(|pid| pid != stopped) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    answering(socket).is_none()
}

fn kill(pid: u32) {
    let killed = sikemux_process::user_environment::command("taskkill")
        .args(["/F", "/PID", &pid.to_string()])
        .output()
        .unwrap();
    assert!(killed.status.success(), "{killed:?}");
}

#[test]
fn a_killed_core_comes_back_and_a_stopped_one_stays_down() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("LOCALAPPDATA", dir.path());
    std::env::set_var(sikemux_process::vars::CONFIG_DIR, dir.path().join("config"));
    let launch = CoreLaunch {
        binary: env!("CARGO_BIN_EXE_sikemux-host").into(),
        socket: dir.path().join("core.sock"),
        log: dir.path().join("logs").join("core.log"),
        args: vec!["--data-dir".into(), dir.path().join("data").into()],
    };
    let file = login_item::install(IDENTIFIER, &launch).unwrap();
    let watching = file.with_extension("watch");
    let log = || std::fs::read_to_string(&launch.log).unwrap_or_default();

    let first = login_item::start_core(IDENTIFIER, &launch).unwrap().pid;
    kill(first);
    let second = back_after(&launch.socket, first)
        .unwrap_or_else(|| panic!("the killed core did not come back\n{}", log()));

    frozen_request(
        &launch.socket,
        &FrozenRequest::StopEverything,
        Duration::from_secs(10),
    )
    .unwrap();
    assert!(
        stays_down(&launch.socket, second),
        "a core told to stop came back\n{}",
        log()
    );
    assert!(!watching.exists(), "{}", log());

    let third = login_item::start_core(IDENTIFIER, &launch).unwrap().pid;
    assert_ne!(third, second);
    login_item::stop_restarting(IDENTIFIER);
    assert!(!watching.exists(), "{}", log());
    kill(third);
    assert!(
        stays_down(&launch.socket, third),
        "a core ended by `stop` came back\n{}",
        log()
    );

    let fourth = ensure_running(&launch.socket, &launch.binary, &launch.log, &launch.args)
        .unwrap()
        .pid;
    login_item::sync(IDENTIFIER, Some(&launch), true);
    kill(fourth);
    back_after(&launch.socket, fourth).unwrap_or_else(|| {
        panic!(
            "a core the app started after `stop` did not come back\n{}",
            log()
        )
    });

    let fifth = login_item::start_core(IDENTIFIER, &launch).unwrap().pid;
    assert!(login_item::remove(IDENTIFIER).unwrap());
    assert!(
        stays_down(&launch.socket, fifth),
        "a removed sign-in entry left its core up\n{}",
        log()
    );
    assert!(!file.exists(), "{}", log());
    assert!(!watching.exists(), "{}", log());
}
