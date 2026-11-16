#![allow(clippy::disallowed_methods)]

use std::os::windows::process::CommandExt;
use std::process::Stdio as StdStdio;
use std::time::Instant;

use sikemux_process::command::Stdio;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn std_command() -> std::process::Command {
    let seam = sikemux_process::user_environment::command("git");
    let mut command = std::process::Command::new(seam.get_program());
    for (key, value) in seam.get_envs() {
        match value {
            Some(value) => command.env(key, value),
            None => command.env_remove(key),
        };
    }
    command.arg("--version").creation_flags(CREATE_NO_WINDOW);
    command
}

fn seam_command() -> sikemux_process::command::Command {
    let mut command = sikemux_process::user_environment::command("git");
    command.arg("--version");
    command
}

fn time(rounds: u32, mut run: impl FnMut()) -> u128 {
    let start = Instant::now();
    for _ in 0..rounds {
        run();
    }
    start.elapsed().as_micros() / u128::from(rounds)
}

#[test]
fn git_spawn_time() {
    let rounds = 200;
    let mut report = String::new();
    for _ in 0..3 {
        let std_null = time(rounds, || {
            let mut c = std_command();
            c.stdin(StdStdio::null())
                .stdout(StdStdio::null())
                .stderr(StdStdio::null());
            assert!(c.status().unwrap().success())
        });
        let seam_null = time(rounds, || {
            let mut c = seam_command();
            c.stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            assert!(c.status().unwrap().success())
        });
        std::env::set_var("SIKEMUX_BENCH_NO_LIST", "1");
        let seam_null_nolist = time(rounds, || {
            let mut c = seam_command();
            c.stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            assert!(c.status().unwrap().success())
        });
        let seam_out_nolist = time(rounds, || {
            assert!(seam_command().output().unwrap().status.success())
        });
        std::env::remove_var("SIKEMUX_BENCH_NO_LIST");
        let std_out = time(rounds, || {
            assert!(std_command().output().unwrap().status.success())
        });
        let seam_out = time(rounds, || {
            assert!(seam_command().output().unwrap().status.success())
        });
        let env_only = time(rounds, || {
            std::hint::black_box(std_command());
        });
        report += &format!(
            "null: std {std_null} seam {seam_null} seam-nolist {seam_null_nolist}; output: std {std_out} seam {seam_out} seam-nolist {seam_out_nolist}; build {env_only} us | "
        );
    }
    panic!("BENCH {report}");
}
