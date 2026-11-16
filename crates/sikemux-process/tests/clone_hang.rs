#![allow(clippy::disallowed_methods)]

use std::io::{Read, Write};

macro_rules! say {
    ($($t:tt)*) => {{
        let _ = writeln!(std::io::stderr(), $($t)*);
    }};
}
use std::time::{Duration, Instant};

fn watchdog(started: Instant) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(40));
        let list = std::process::Command::new("tasklist")
            .arg("/v")
            .output()
            .unwrap();
        say!(
            "HANG at {:?}\n{}",
            started.elapsed(),
            String::from_utf8_lossy(&list.stdout)
        );
        std::process::exit(3);
    });
}

#[test]
fn clone_that_cannot_connect() {
    let started = Instant::now();
    watchdog(started);
    let into = std::env::temp_dir().join(format!("clone-hang-{}", std::process::id()));
    let mut command = sikemux_process::user_environment::command("git");
    command
        .args([
            "clone",
            "--depth",
            "1",
            "--quiet",
            "https://127.0.0.1:9/skill.git",
        ])
        .arg(&into)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(sikemux_process::command::Stdio::null())
        .stdout(sikemux_process::command::Stdio::null())
        .stderr(sikemux_process::command::Stdio::piped());
    say!("PHASE program {:?}", command.get_program());
    let mut child = command.spawn().unwrap();
    say!(
        "PHASE spawned pid {} at {:?}",
        child.id(),
        started.elapsed()
    );
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    say!("PHASE exited {status:?} at {:?}", started.elapsed());
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    say!("PHASE stderr read at {:?}: {stderr}", started.elapsed());
    panic!("DONE in {:?}", started.elapsed());
}
