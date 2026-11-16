#![allow(clippy::disallowed_methods)]

use std::io::{Read, Write};
use std::os::windows::process::CommandExt;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use sikemux_process::command::Stdio;

macro_rules! say {
    ($($t:tt)*) => {{
        let _ = writeln!(std::io::stderr(), $($t)*);
    }};
}

const URL: &str = "https://127.0.0.1:9/skill.git";

fn finish(name: &str, started: Instant, pipe: Box<dyn Read + Send>) {
    let (send, receive) = mpsc::channel();
    let mut pipe = pipe;
    std::thread::spawn(move || {
        let mut text = String::new();
        let result = pipe.read_to_string(&mut text);
        let _ = send.send((result.map(|_| text), Instant::now()));
    });
    match receive.recv_timeout(Duration::from_secs(15)) {
        Ok((result, at)) => say!("CASE {name}: read at {:?}: {result:?}", at - started),
        Err(_) => say!("CASE {name}: HANG"),
    }
}

fn seam(name: &str, args: &[&str], stdin_stdout_null: bool) {
    let started = Instant::now();
    let mut command = sikemux_process::user_environment::command("git");
    command.args(args).env("GIT_TERMINAL_PROMPT", "0");
    if stdin_stdout_null {
        command.stdin(Stdio::null()).stdout(Stdio::null());
    }
    command.stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    say!("CASE {name}: exited {status:?} at {:?}", started.elapsed());
    finish(name, started, Box::new(child.stderr.take().unwrap()));
}

fn standard(name: &str, args: &[&str], no_window: bool) {
    let started = Instant::now();
    let program = sikemux_process::user_environment::command("git")
        .get_program()
        .to_owned();
    let mut command = std::process::Command::new(program);
    command
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    if no_window {
        command.creation_flags(0x0800_0000);
    }
    let mut child = command.spawn().unwrap();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    say!("CASE {name}: exited {status:?} at {:?}", started.elapsed());
    finish(name, started, Box::new(child.stderr.take().unwrap()));
}

#[test]
fn where_the_pipe_stays_open() {
    let into = std::env::temp_dir().join(format!("clone-hang-{}", std::process::id()));
    let into = into.to_str().unwrap().to_owned();
    let clone = ["clone", "--depth", "1", "--quiet", URL, into.as_str()];
    seam("seam-version", &["--version"], true);
    seam("seam-bad-arg", &["no-such-command"], true);
    seam("seam-ls-remote", &["ls-remote", URL], true);
    seam("seam-clone", &clone, true);
    seam("seam-clone-inherit", &clone, false);
    standard("std-clone", &clone, false);
    standard("std-clone-no-window", &clone, true);
    let list = std::process::Command::new("tasklist")
        .arg("/v")
        .output()
        .unwrap();
    say!("TASKS\n{}", String::from_utf8_lossy(&list.stdout));
    std::process::exit(0);
}
