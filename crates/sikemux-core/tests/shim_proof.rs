use std::path::Path;

fn git(shims: &Path, repo: &Path, agent: bool, args: &[&str]) -> std::process::Output {
    let path = std::env::var_os("PATH").unwrap();
    let path = std::env::join_paths(
        std::iter::once(shims.to_path_buf()).chain(std::env::split_paths(&path)),
    )
    .unwrap();
    let mut command = sikemux_process::command::Command::new("git");
    command.args(args).current_dir(repo).env("PATH", path);
    command.env_remove("SIKEMUX_AGENT_ID");
    command.env_remove(sikemux_process::vars::BUILD_TURN);
    for name in [
        "SIKEMUX_CLI_ENDPOINT",
        "SIKEMUX_AGENT_HOOKS",
        "SIKEMUX_REAL_GIT",
    ] {
        command.env_remove(name);
    }
    if agent {
        command.env("SIKEMUX_AGENT_ID", "proof");
    }
    let output = command.output().unwrap();
    eprintln!(
        "git {args:?} agent={agent}: {:?}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    std::process::Output {
        status: output.status,
        stdout: output.stdout,
        stderr: output.stderr,
    }
}

#[test]
fn the_core_linked_as_git_guards_an_agents_git() {
    let top = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let shims = top.path().join("agent-bin");
    std::fs::create_dir(&shims).unwrap();
    let shim = shims.join(format!("git{}", std::env::consts::EXE_SUFFIX));
    std::fs::hard_link(env!("CARGO_BIN_EXE_sikemux-core"), &shim).unwrap();
    let repo = top.path().join("repo");
    std::fs::create_dir(&repo).unwrap();

    let version = git(&shims, &repo, false, &["--version"]);
    assert!(version.status.success());
    assert!(String::from_utf8_lossy(&version.stdout).starts_with("git version"));

    for args in [
        &["init", "-q"][..],
        &[
            "-c",
            "user.name=p",
            "-c",
            "user.email=p@p",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "a",
        ],
    ] {
        assert!(git(&shims, &repo, false, args).status.success());
    }
    std::fs::write(repo.join("f.txt"), "one\n").unwrap();
    assert!(git(&shims, &repo, false, &["add", "f.txt"])
        .status
        .success());
    assert!(git(
        &shims,
        &repo,
        false,
        &[
            "-c",
            "user.name=p",
            "-c",
            "user.email=p@p",
            "commit",
            "-q",
            "-m",
            "b"
        ]
    )
    .status
    .success());
    std::fs::write(repo.join("f.txt"), "two\n").unwrap();

    let status = git(&shims, &repo, true, &["status", "--short"]);
    assert!(status.status.success());
    assert!(String::from_utf8_lossy(&status.stdout).contains("f.txt"));

    let stash = git(&shims, &repo, true, &["stash"]);
    assert_eq!(stash.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&stash.stderr).contains("Sikemux refused"));
    assert_eq!(
        std::fs::read_to_string(repo.join("f.txt")).unwrap(),
        "two\n"
    );
}
