#![allow(clippy::disallowed_methods)]

use std::os::windows::process::CommandExt;
use std::time::Instant;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn std_command(flags: u32) -> std::process::Command {
    let seam = sikemux_process::user_environment::command("git");
    let mut command = std::process::Command::new(seam.get_program());
    for (key, value) in seam.get_envs() {
        match value {
            Some(value) => command.env(key, value),
            None => command.env_remove(key),
        };
    }
    command.arg("--version").creation_flags(flags);
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
        let bare = time(rounds, || {
            assert!(std_command(0).output().unwrap().status.success())
        });
        let std = time(rounds, || {
            assert!(std_command(CREATE_NO_WINDOW)
                .output()
                .unwrap()
                .status
                .success())
        });
        let seam = time(rounds, || {
            let out = sikemux_process::user_environment::command("git")
                .arg("--version")
                .output()
                .unwrap();
            assert!(out.status.success());
        });
        let std_null = time(rounds, || {
            let mut command = std_command(CREATE_NO_WINDOW);
            command.stderr(std::process::Stdio::null());
            assert!(command.output().unwrap().status.success())
        });
        let seam_null = time(rounds, || {
            let out = sikemux_process::user_environment::command("git")
                .arg("--version")
                .stderr(sikemux_process::command::Stdio::null())
                .output()
                .unwrap();
            assert!(out.status.success());
        });
        let thread = time(rounds, || std::thread::spawn(|| ()).join().unwrap());
        report += &format!(
            "std-console {bare}, std {std}, seam {seam}, std-null-stderr {std_null}, seam-null-stderr {seam_null}, thread {thread} us; "
        );
    }
    panic!("BENCH {report}");
}
