use std::time::Instant;

#[test]
fn git_spawn_time() {
    let rounds = 200;
    let git = sikemux_process::user_environment::git_program().unwrap_or("git".into());
    let mut report = String::new();
    for _ in 0..3 {
        let start = Instant::now();
        for _ in 0..rounds {
            #[allow(clippy::disallowed_methods)]
            let out = std::process::Command::new(&git)
                .arg("--version")
                .output()
                .unwrap();
            assert!(out.status.success());
        }
        let std_us = start.elapsed().as_micros() / rounds;
        let start = Instant::now();
        for _ in 0..rounds {
            let out = sikemux_process::user_environment::command("git")
                .arg("--version")
                .output()
                .unwrap();
            assert!(out.status.success());
        }
        let seam_us = start.elapsed().as_micros() / rounds;
        report += &format!("std {std_us} us, seam {seam_us} us; ");
    }
    panic!("BENCH {report}");
}
