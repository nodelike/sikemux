#![allow(clippy::disallowed_methods)]

use std::io::{Read, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use sikemux_process::command::{Command, Stdio};

macro_rules! say {
    ($($t:tt)*) => {{
        let _ = writeln!(std::io::stderr(), $($t)*);
    }};
}

const URL: &str = "https://127.0.0.1:9/skill.git";

fn finish(name: &str, started: Instant, mut pipe: Box<dyn Read + Send>) {
    let (send, receive) = mpsc::channel();
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

fn run_seam(name: &str, mut command: Command) {
    let started = Instant::now();
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    let status = child.wait().unwrap();
    say!("CASE {name}: exited {status:?} at {:?}", started.elapsed());
    finish(name, started, Box::new(child.stderr.take().unwrap()));
}

fn user_git() -> Command {
    let mut command = sikemux_process::user_environment::command("git");
    command.args(["ls-remote", URL]);
    command
}

#[test]
fn where_the_pipe_stays_open() {
    let git = user_git().get_program().to_owned();
    let user_env: Vec<_> = user_git()
        .get_envs()
        .map(|(key, value)| (key.to_owned(), value.map(|v| v.to_owned())))
        .collect();
    let mut differ = Vec::new();
    for (key, value) in &user_env {
        if std::env::var_os(key) != *value {
            differ.push(key.to_string_lossy().into_owned());
        }
    }
    say!("ENV differs from parent: {differ:?}");

    run_seam("seam-ls-remote", user_git());

    let mut plain = Command::new(&git);
    plain.args(["ls-remote", URL]);
    run_seam("seam-parent-env", plain);

    let mut echo = Command::new("cmd");
    echo.args(["/c", "echo x 1>&2"]);
    run_seam("seam-cmd-echo", echo);

    let mut nested = Command::new("cmd");
    nested.args(["/c", "cmd /c echo x 1>&2"]);
    run_seam("seam-cmd-nested", nested);

    let started = Instant::now();
    let mut command = std::process::Command::new(&git);
    command.args(["ls-remote", URL]);
    for (key, value) in &user_env {
        match value {
            Some(value) => command.env(key, value),
            None => command.env_remove(key),
        };
    }
    let mut child = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let status = child.wait().unwrap();
    say!(
        "CASE std-user-env: exited {status:?} at {:?}",
        started.elapsed()
    );
    finish(
        "std-user-env",
        started,
        Box::new(child.stderr.take().unwrap()),
    );

    unsafe { std::env::set_var("PROOF_NO_HANDLE_LIST", "1") };
    run_seam("seam-no-handle-list", user_git());
    std::process::exit(0);
}
