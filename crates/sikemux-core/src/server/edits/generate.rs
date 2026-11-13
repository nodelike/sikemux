//! Keeps the files a project builds from its other files, and commits, up to
//! date on its own. After a commit lands, its project's `generate` command
//! runs in the integration checkout; when the files it writes come out
//! different, the core commits them on top, credited to the commit's agents,
//! and moves the project's checkout to it as a landing does. Agents never run
//! the generators in a checkout they share, where they would read the other
//! agents' work in progress.

use crate::server::remote_state::RemoteParts;
use crate::server::repo_state::RepoParts;
use std::path::Path;
use std::time::Duration;
use uuid::Uuid;

use super::footer::known;
use super::land::{apply, steps};
use super::owners::{git, run};
use super::push::line;
use crate::server::trust_ask::TrustAsk;
use crate::server::Core;
use crate::tasks::config::{self, ConfigLoad, GenerateConfig};
use crate::trailers::{self, Agent, Commit};

const GENERATE_WAIT: Duration = Duration::from_secs(10 * 60);
const GENERATE_OUTPUT_LIMIT: usize = 256 * 1024;

/// How the subject of a commit that brings generated files up to date starts.
pub(crate) const GENERATED_SUBJECT: &str = "build(gen): ";

pub(crate) const UNTRUSTED: &str =
    "sikemux.json is not trusted yet, so its generate command did not run";

/// What the person must trust before the `generate` command `checkout`'s `sikemux.json` names
/// runs, or none. They trust a project's file, never the integration checkout's, so the grant is
/// looked up by the project's path and the content of the file at this commit: a commit that
/// changes the file asks again.
pub(crate) fn trust_needed(core: &Core, project: &Path, checkout: &Path) -> Option<TrustAsk> {
    let ConfigLoad::Valid {
        config,
        fingerprint,
        trust,
        ..
    } = config::load(checkout)
    else {
        return None;
    };
    config.generate.as_ref()?;
    (core.repo_above().tasks_config_needs)(
        core,
        &ConfigLoad::Valid {
            path: project
                .join(config::CONFIG_FILE)
                .to_string_lossy()
                .into_owned(),
            config,
            fingerprint,
            trust,
        },
    )
}

/// Runs the project's `generate` command in `checkout`, the integration
/// checkout at `commit` of the branch `project`'s checkout is on, and commits
/// what it changed of its outputs, credited to `agents`. The commit made, if
/// the outputs changed and the branch had not moved meanwhile.
pub(crate) fn regenerate(
    core: &Core,
    project: &Path,
    checkout: &Path,
    commit: &str,
    agents: &[String],
) -> Result<Option<String>, String> {
    if trust_needed(core, project, checkout).is_some() {
        return Err(UNTRUSTED.to_owned());
    }
    let ConfigLoad::Valid { config: found, .. } = config::load(checkout) else {
        return Ok(None);
    };
    let Some(generate) = found.generate.clone() else {
        return Ok(None);
    };
    let signers: Vec<_> = agents
        .iter()
        .filter_map(|agent| known(core, project, agent))
        .collect();
    let key = core.remote().host_key();
    bring_up_to_date(project, checkout, commit, &generate, &signers, key.as_ref())
}

/// Brings the files the project generates up to date in `checkout`, committing nothing, for
/// checking a commit that may have left them stale.
pub(crate) fn refresh(core: &Core, project: &Path, checkout: &Path) -> Result<(), String> {
    if trust_needed(core, project, checkout).is_some() {
        return Err(UNTRUSTED.to_owned());
    }
    let ConfigLoad::Valid { config: found, .. } = config::load(checkout) else {
        return Ok(());
    };
    match &found.generate {
        Some(generate) => run_generate(checkout, generate),
        None => Ok(()),
    }
}

fn run_generate(checkout: &Path, generate: &GenerateConfig) -> Result<(), String> {
    let mut shell = sikemux_process::user_environment::background_shell();
    shell.args(["-c", &generate.command]).current_dir(checkout);
    let ran = sikemux_process::run(&mut shell, None, GENERATE_WAIT, GENERATE_OUTPUT_LIMIT, None)
        .map_err(|error| format!("generate did not run: {error}"))?;
    if !ran.status.success() {
        return Err(format!(
            "generate failed: {}",
            String::from_utf8_lossy(&ran.stderr).trim()
        ));
    }
    Ok(())
}

/// Runs `generate` in `checkout` at `commit`, and commits on top what it
/// changed of its outputs, signed for `signers` with `key`, moving the branch
/// `project`'s checkout is on and its files.
fn bring_up_to_date(
    project: &Path,
    checkout: &Path,
    commit: &str,
    generate: &GenerateConfig,
    signers: &[Agent],
    key: Option<&iroh::SecretKey>,
) -> Result<Option<String>, String> {
    run_generate(checkout, generate)?;
    let specs: Vec<String> = generate
        .outputs
        .iter()
        .map(|output| format!(":(literal){output}"))
        .collect();
    let mut status = vec!["status", "--porcelain", "-z", "--"];
    status.extend(specs.iter().map(String::as_str));
    if git(checkout, &status)?.is_empty() {
        return Ok(None);
    }
    let git_dir = line(git(checkout, &["rev-parse", "--absolute-git-dir"])?);
    let index = Path::new(&git_dir).join(format!(
        "sikemux-generate-{}.index",
        Uuid::new_v4().simple()
    ));
    let made = {
        let private = Some(index.as_path());
        let built = run(checkout, &["read-tree", commit], private, None).and_then(|_| {
            let mut add = vec!["add", "--"];
            add.extend(specs.iter().map(String::as_str));
            run(checkout, &add, private, None)?;
            run(checkout, &["write-tree"], private, None)
        });
        let _ = std::fs::remove_file(&index);
        line(String::from_utf8_lossy(&built?).into_owned())
    };
    let parents = vec![commit.to_owned()];
    let subject = format!(
        "{GENERATED_SUBJECT}{} brought up to date",
        generate.outputs.join(", ")
    );
    let footer = trailers::footer(
        signers,
        &Commit {
            tree: &made,
            parents: &parents,
        },
        key,
    );
    let message = trailers::append(&subject, &footer);
    let new = line(
        String::from_utf8_lossy(&run(
            checkout,
            &["commit-tree", &made, "-p", commit, "-F", "-"],
            None,
            Some(message.as_bytes()),
        )?)
        .into_owned(),
    );
    let branch = line(git(project, &["symbolic-ref", "-q", "HEAD"])?);
    let ready = steps(project, commit, &new)?;
    run(
        project,
        &["update-ref", "-m", &subject, &branch, &new, commit],
        None,
        None,
    )
    .map_err(|_| "the branch moved while generating; the next commit regenerates".to_owned())?;
    apply(project, ready, commit, &new);
    git(
        checkout,
        &["checkout", "--detach", "--force", "--quiet", &new],
    )?;
    Ok(Some(new))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::edits::tests_git::git_in;
    use crate::server::ServerCore;

    #[test]
    fn stale_outputs_are_committed_on_top_and_reach_the_project_checkout() {
        let root = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(root.path()).unwrap();
        let main = root.join("project");
        std::fs::create_dir(&main).unwrap();
        git_in(&main, &["init", "-q", "-b", "main"]);
        std::fs::write(main.join("schema.txt"), "two\n").unwrap();
        std::fs::write(main.join("generated.txt"), "one\n").unwrap();
        std::fs::write(main.join("notes.txt"), "mine\n").unwrap();
        git_in(&main, &["add", "."]);
        git_in(&main, &["commit", "-qm", "base"]);
        std::fs::write(main.join("notes.txt"), "mine, not committed\n").unwrap();
        let head = git_in(&main, &["rev-parse", "HEAD"]);
        let checkout = root.join("integration");
        git_in(
            &main,
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                &checkout.to_string_lossy(),
                &head,
            ],
        );
        git_in(&checkout, &["config", "user.name", "t"]);
        git_in(&checkout, &["config", "user.email", "t@t"]);
        let generate = GenerateConfig {
            command: "cp schema.txt generated.txt".into(),
            outputs: vec!["generated.txt".into()],
        };
        let made = bring_up_to_date(&main, &checkout, &head, &generate, &[], None)
            .unwrap()
            .unwrap();
        assert_eq!(git_in(&main, &["rev-parse", "main"]), made);
        assert_eq!(git_in(&main, &["rev-parse", "main^"]), head);
        assert_eq!(
            std::fs::read_to_string(main.join("generated.txt")).unwrap(),
            "two\n"
        );
        assert_eq!(
            std::fs::read_to_string(main.join("notes.txt")).unwrap(),
            "mine, not committed\n"
        );
        assert_eq!(git_in(&main, &["status", "--porcelain"]), "M notes.txt");
        assert!(
            bring_up_to_date(&main, &checkout, &made, &generate, &[], None)
                .unwrap()
                .is_none()
        );
    }

    /// On 2026-11-07 the check of a slice in this project kept refusing with "sikemux.json is not
    /// trusted yet" after the person trusted it: the grant named the project's file, and this
    /// looked one up for the integration checkout's, which nobody can be asked about.
    #[tokio::test]
    async fn a_trusted_file_runs_from_the_integration_checkout_and_a_changed_one_does_not() {
        let root = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(root.path()).unwrap();
        let main = root.join("project");
        std::fs::create_dir(&main).unwrap();
        git_in(&main, &["init", "-q", "-b", "main"]);
        git_in(&main, &["config", "user.name", "t"]);
        git_in(&main, &["config", "user.email", "t@t"]);
        let settings = |command: &str| {
            format!(
                r#"{{"version":1,"generate":{{"command":"{command}","outputs":["generated.txt"]}}}}"#
            )
        };
        std::fs::write(
            main.join("sikemux.json"),
            settings("cp schema.txt generated.txt"),
        )
        .unwrap();
        std::fs::write(main.join("schema.txt"), "two\n").unwrap();
        std::fs::write(main.join("generated.txt"), "one\n").unwrap();
        git_in(&main, &["add", "."]);
        git_in(&main, &["commit", "-qm", "base"]);
        let head = git_in(&main, &["rev-parse", "HEAD"]);
        let checkout = root.join("integration");
        git_in(
            &main,
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                &checkout.to_string_lossy(),
                &head,
            ],
        );
        git_in(&checkout, &["config", "user.name", "t"]);
        git_in(&checkout, &["config", "user.email", "t@t"]);
        let core = Core::new(Default::default(), None).unwrap();
        assert_eq!(
            regenerate(&core, &main, &checkout, &head, &[]).unwrap_err(),
            UNTRUSTED
        );
        let ConfigLoad::Valid { fingerprint, .. } = config::load(&main) else {
            panic!("the project's file is valid");
        };
        let project = main.to_string_lossy().into_owned();
        crate::server::tasks::trust_project(&core, project, fingerprint, None)
            .await
            .unwrap();

        let made = regenerate(&core, &main, &checkout, &head, &[])
            .unwrap()
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(main.join("generated.txt")).unwrap(),
            "two\n"
        );

        std::fs::write(
            main.join("sikemux.json"),
            settings("echo changed > generated.txt"),
        )
        .unwrap();
        git_in(&main, &["add", "sikemux.json"]);
        git_in(&main, &["commit", "-qm", "changes the command"]);
        let changed = git_in(&main, &["rev-parse", "HEAD"]);
        assert_ne!(changed, made);
        git_in(&checkout, &["checkout", "-q", "--detach", &changed]);
        assert_eq!(
            regenerate(&core, &main, &checkout, &changed, &[]).unwrap_err(),
            UNTRUSTED
        );
        assert!(trust_needed(&core, &main, &checkout).is_some());
        assert_eq!(
            std::fs::read_to_string(main.join("generated.txt")).unwrap(),
            "two\n"
        );
    }
}
