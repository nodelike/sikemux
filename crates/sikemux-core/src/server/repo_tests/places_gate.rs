#![allow(unused_imports)]

use crate::server::changed::ChangedCore;
use crate::server::changed::{Topic, What};
use crate::server::connection::blocking;
use crate::server::places::judge::{Goes, Judged};
use crate::server::places::place::{Place, GITHUB_RELEASES};
use crate::server::{Core, CoreResult};
use crate::ship::Ship;
use serde::{Deserialize, Serialize};
use sikemux_calls::places::{AddPlaceParams, PlaceLevelParams, PlacesParams, SeenParams};
use sikemux_process::clock::epoch_ms;
use sikemux_view::calls::CallKind;
use sikemux_view::memory::Learned;
use sikemux_view::places::{Places, Seen};

use crate::server::remote_state::RemoteParts;
use crate::server::repo_state::RepoParts;
use crate::server::workspace_state::WorkspaceParts;
use crate::server::ServerCore;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sikemux_view::calls::{CallResult, CallState, PersonCall};
use sikemux_view::inbox::ShipKind;
use sikemux_view::places::{Level, Recognise};
use sikemux_view::slice::Verdict;

use crate::server::access::Peer;
use crate::server::places::place::new_place;
use crate::server::places::push::tests::git;
use crate::server::places::*;
use crate::server::ship::{self, Deploy, HoldParams};
use crate::ship::Target;

/// A clone on branch `work` with one change against main, as an agent's checkout.
fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let origin = dir.path().join("origin.git");
    let work = dir.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    git(
        dir.path(),
        &["init", "--bare", "-b", "main", origin.to_str().unwrap()],
    );
    git(&work, &["init", "-b", "main"]);
    std::fs::write(work.join("app.txt"), "one\n").unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "-m", "first"]);
    git(
        &work,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&work, &["push", "-u", "origin", "main"]);
    git(&work, &["remote", "set-head", "origin", "main"]);
    git(&work, &["checkout", "-b", "work"]);
    std::fs::write(work.join("app.txt"), "two\n").unwrap();
    git(&work, &["commit", "-am", "two"]);
    dir
}

fn vercel_place() -> Place {
    let fact = Recognise::VercelTarget { production: true };
    new_place("acme.dev on Vercel", true, vec![fact], None)
}

fn database_place() -> Place {
    let fact = Recognise::Database {
        name: "main".into(),
    };
    new_place("The main database", true, vec![fact], None)
}

/// A core whose agent `agent` works in `project`, which keeps `places`, confirmed.
fn core_at(data: Option<&Path>, project: &str, places: Vec<Place>) -> Arc<Core> {
    let core = Core::new(Default::default(), data).unwrap();
    let adopted = crate::server::names::Adopted {
        agent_id: "agent",
        name: "Ada",
        project: project.to_owned(),
        parent: None,
        provider: "claude",
        reports: false,
        task: None,
    };
    core.names().adopt(adopted, ["agent".to_owned()].into());
    change(&core, project, |kept| {
        kept.places = places;
        kept.confirmed = Some(1);
        Ok(())
    })
    .unwrap();
    core
}

fn deploy(what: &str) -> Ship {
    Ship {
        kind: ShipKind::Deploy,
        what: what.to_owned(),
        target: Some(Target::Vercel { production: true }),
    }
}

fn write(sql: &str) -> Ship {
    Ship {
        kind: ShipKind::ProductionWrite,
        what: format!("db_query {sql}"),
        target: Some(Target::Database {
            name: "main".into(),
        }),
    }
}

/// A release workflow run by pushed tags that tells its nightly tags apart from the rest.
const TAG_RELEASES: &str = r#"name: Release
on:
  push:
    tags: ["v*"]
jobs:
  publish:
    runs-on: ubuntu-latest
    steps:
      - run: gh release create "$GITHUB_REF_NAME" --prerelease
        if: ${{ contains(github.ref_name, '-nightly.') }}
      - run: gh release create "$GITHUB_REF_NAME" --latest
        if: ${{ !contains(github.ref_name, '-nightly.') }}
"#;

fn run(core: &Core, ship: Ship, cwd: &Path) -> Result<Option<Deploy>, String> {
    ship::check(core, "agent", ship, cwd)
}

fn open_calls(core: &Core) -> Vec<PersonCall> {
    (crate::server::person_calls::shown(core).into_iter())
        .filter(|call| call.state == CallState::Open)
        .collect()
}

async fn answer(core: &Arc<Core>, id: &str, option: &str) {
    let hold = HoldParams {
        hold: id.to_owned(),
    };
    crate::server::person_calls::answer_hold(core, &Peer::App, hold, option)
        .await
        .unwrap();
}

fn work_of(dir: &tempfile::TempDir) -> (PathBuf, String) {
    let work = dir.path().join("work");
    (work.clone(), work.to_string_lossy().into_owned())
}

/// The person approved one statement on a production database, and every later write ran.
#[tokio::test(flavor = "multi_thread")]
async fn a_yes_covers_any_other_action_once_exactly_as_asked() {
    let dir = repo();
    let (cwd, project) = work_of(&dir);
    let core = core_at(None, &project, vec![database_place()]);
    assert!(run(&core, write("UPDATE users SET a=1 WHERE id=5"), &cwd).is_err());
    let call = open_calls(&core).remove(0);
    answer(&core, &call.id, "allow").await;
    assert!(run(&core, write("DROP TABLE users"), &cwd).is_err());
    assert!(run(&core, write("UPDATE users SET a=1 WHERE id=5"), &cwd).is_ok());
    assert!(run(&core, write("UPDATE users SET a=1 WHERE id=5"), &cwd).is_err());
}

/// Work changed while its call waited, and every tap was refused with nobody told.
#[tokio::test(flavor = "multi_thread")]
async fn a_call_whose_work_changed_is_withdrawn_and_the_next_try_asks_again() {
    let dir = repo();
    let (cwd, project) = work_of(&dir);
    let core = core_at(None, &project, vec![vercel_place()]);
    assert!(run(&core, deploy("vercel --prod"), &cwd).is_err());
    let first = open_calls(&core).remove(0);
    std::fs::write(cwd.join("new.txt"), "x").unwrap();
    assert!(run(&core, deploy("vercel --prod"), &cwd).is_err());
    let open = open_calls(&core);
    assert_eq!(open.len(), 1);
    assert_ne!(open[0].id, first.id);
    let withdrawn = (crate::server::person_calls::shown(&core).into_iter())
        .find(|call| call.id == first.id)
        .unwrap();
    assert_eq!(
        withdrawn.outcome.map(|outcome| outcome.result),
        Some(CallResult::Withdrawn)
    );
    answer(&core, &open[0].id, "refuse").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn one_agent_waits_on_no_more_calls_than_sikemux_keeps() {
    let core = core_at(None, "/tmp/sikemux-gate-cap", vec![database_place()]);
    for at in 0..40 {
        let _ = run(
            &core,
            write(&format!("UPDATE t SET a={at}")),
            Path::new("/nowhere"),
        );
    }
    assert!(open_calls(&core).len() <= 16, "{}", open_calls(&core).len());
}

/// Each command of a deploy at "tell me" raised a call of its own, born shipped.
#[tokio::test(flavor = "multi_thread")]
async fn tell_me_tells_once_per_deploy_when_it_ends() {
    let dir = repo();
    let (cwd, project) = work_of(&dir);
    let mut place = vercel_place();
    place.shown.level = Level::TellMe;
    let core = core_at(None, &project, vec![place]);
    let first = run(&core, deploy("vercel --prod"), &cwd).unwrap().unwrap();
    let second = run(&core, deploy("vercel deploy --prod"), &cwd)
        .unwrap()
        .unwrap();
    assert!(crate::server::person_calls::shown(&core).is_empty());
    ship::ended(core.clone(), first, Some(Verdict::Failed)).await;
    ship::ended(core.clone(), second, Some(Verdict::Passed)).await;
    let told = crate::server::person_calls::shown(&core);
    assert_eq!(told.len(), 1);
    assert_eq!(
        told[0].outcome.as_ref().map(|outcome| outcome.result),
        Some(CallResult::Shipped)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_yes_from_before_a_restart_passes_nothing() {
    let dir = repo();
    let data = tempfile::tempdir().unwrap();
    let (cwd, project) = work_of(&dir);
    let core = core_at(Some(data.path()), &project, vec![vercel_place()]);
    assert!(run(&core, deploy("vercel --prod"), &cwd).is_err());
    let call = open_calls(&core).remove(0);
    answer(&core, &call.id, "allow").await;
    drop(core);
    std::thread::sleep(std::time::Duration::from_millis(5));
    let again = core_at(Some(data.path()), &project, vec![vercel_place()]);
    assert!(run(&again, deploy("vercel --prod"), &cwd).is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_yes_covers_neither_another_place_nor_another_project() {
    let dir = repo();
    let (cwd, project) = work_of(&dir);
    let core = core_at(None, &project, vec![vercel_place(), database_place()]);
    assert!(run(&core, deploy("vercel --prod"), &cwd).is_err());
    let call = open_calls(&core).remove(0);
    answer(&core, &call.id, "allow").await;
    assert!(run(&core, write("UPDATE t SET a=1"), &cwd).is_err());
    let elsewhere = core_at(None, "/tmp/sikemux-gate-elsewhere", vec![vercel_place()]);
    let calls = crate::server::person_calls::shown(&core);
    assert!(calls.iter().all(|call| call.project == project));
    assert!(run(&elsewhere, deploy("vercel --prod"), &cwd).is_err());
}

/// `cd ~ && kubectl --context staging apply` escaped a context the person counted.
#[test]
fn places_are_the_agents_project_not_the_commands_folder() {
    let counted = new_place(
        "Kubernetes context staging",
        true,
        vec![Recognise::KubeContext {
            name: "staging".into(),
        }],
        None,
    );
    let core = core_at(None, "/tmp/sikemux-gate-project", vec![counted]);
    let ship = Ship {
        kind: ShipKind::Deploy,
        what: "kubectl --context staging apply -f x".into(),
        target: Some(Target::KubeContext {
            name: "staging".into(),
        }),
    };
    assert!(run(&core, ship, Path::new("/tmp")).is_err());
}

/// One `terraform apply` with no workspace kept a place for the bare word, which then held a
/// dev workspace and, moved to yours, freed a production one.
#[test]
fn a_target_with_no_name_keeps_no_place() {
    let core = core_at(None, "/tmp/sikemux-gate-unnamed", Vec::new());
    let bare = Ship {
        kind: ShipKind::Deploy,
        what: "terraform apply".into(),
        target: None,
    };
    assert!(run(&core, bare, Path::new("/nowhere")).is_ok());
    assert!(core
        .places()
        .of("/tmp/sikemux-gate-unnamed")
        .places
        .is_empty());
    let dev = Ship {
        kind: ShipKind::Deploy,
        what: "terraform apply".into(),
        target: Some(Target::TerraformWorkspace { name: "dev".into() }),
    };
    assert_eq!(
        run(&core, dev, Path::new("/nowhere")).map(|deploy| deploy.is_some()),
        Ok(false)
    );
}

/// An agent writing to more sites than the list holds pushed out what the person hadn't seen.
#[test]
fn the_seen_list_never_drops_what_the_person_has_not_looked_at() {
    let project = "/tmp/sikemux-gate-seen";
    let core = core_at(None, project, Vec::new());
    for at in 0..70 {
        let ship = Ship {
            kind: ShipKind::Publish,
            what: format!("curl -d x https://site{at}.example.com"),
            target: Some(Target::Site {
                origin: format!("https://site{at}.example.com"),
            }),
        };
        assert!(run(&core, ship, Path::new("/nowhere")).is_ok());
    }
    let read = of(&core, project);
    assert_eq!(read.seen.len(), 64);
    assert_eq!(read.seen[0].what, "site0.example.com");
    assert_eq!(read.more_seen, 6);
}

/// A deploy's yes ended at each command that exited well, so three `kubectl apply` in a row
/// were three cards.
#[tokio::test(flavor = "multi_thread")]
async fn a_deploys_yes_covers_every_command_of_it_until_the_turn_ends() {
    let dir = repo();
    let (cwd, project) = work_of(&dir);
    let core = core_at(None, &project, vec![vercel_place()]);
    assert!(run(&core, deploy("vercel --prod"), &cwd).is_err());
    let call = open_calls(&core).remove(0);
    answer(&core, &call.id, "allow").await;
    core.ship().turn_started("agent");
    for what in [
        "vercel --prod",
        "vercel deploy --prod --yes",
        "vercel --prod",
    ] {
        let went = run(&core, deploy(what), &cwd).unwrap().unwrap();
        ship::ended(core.clone(), went, Some(Verdict::Passed)).await;
    }
    let unseen = run(&core, deploy("vercel --prod"), &cwd).unwrap().unwrap();
    ship::ended(core.clone(), unseen, None).await;
    assert!(run(&core, deploy("vercel --prod"), &cwd).unwrap().is_some());
    core.ship().turn_ended("agent");
    assert!(run(&core, deploy("vercel --prod"), &cwd).is_err());
}

/// A no ended with the turn, and the agent's retry in its next turn raised a new card.
#[tokio::test(flavor = "multi_thread")]
async fn a_no_holds_until_the_work_changes_or_the_person_speaks() {
    let dir = repo();
    let (cwd, project) = work_of(&dir);
    let core = core_at(None, &project, vec![vercel_place()]);
    assert!(run(&core, deploy("vercel --prod"), &cwd).is_err());
    let call = open_calls(&core).remove(0);
    answer(&core, &call.id, "refuse").await;
    core.ship().turn_started("agent");
    core.ship().turn_ended("agent");
    let said = run(&core, deploy("vercel deploy --prod"), &cwd)
        .err()
        .unwrap_or_default();
    assert!(said.starts_with("Not yet"), "{said}");
    assert!(open_calls(&core).is_empty());
    std::thread::sleep(std::time::Duration::from_millis(2));
    core.ship().person_spoke("agent");
    assert!(run(&core, deploy("vercel --prod"), &cwd).is_err());
    assert_eq!(open_calls(&core).len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_deploy_at_tell_me_nobody_saw_end_may_have_shipped() {
    let dir = repo();
    let (cwd, project) = work_of(&dir);
    let mut place = vercel_place();
    place.shown.level = Level::TellMe;
    let core = core_at(None, &project, vec![place]);
    let went = run(&core, deploy("vercel --prod"), &cwd).unwrap().unwrap();
    ship::ended(core.clone(), went, None).await;
    let told = crate::server::person_calls::shown(&core);
    assert_eq!(
        told[0].outcome.as_ref().map(|outcome| outcome.result),
        Some(CallResult::MayHaveRun)
    );
}

/// A tool call's deploy was covered by its target, so a yes to promote one build promoted
/// another.
#[test]
fn a_tool_call_is_covered_once_exactly() {
    let place = vercel_place();
    let promote = |id: &str| Ship {
        kind: ShipKind::Deploy,
        what: format!("vercel_promote {{\"id\":\"{id}\"}}"),
        target: Some(Target::Vercel { production: true }),
    };
    let (one, once) = ship::key_of(&place, &promote("dpl_A"), Some("work"), None, true);
    let (other, _) = ship::key_of(&place, &promote("dpl_B"), Some("work"), None, true);
    assert!(once);
    assert_ne!(one, other);
    let (command, once) = ship::key_of(&place, &deploy("vercel --prod"), Some("work"), None, false);
    assert!(!once);
    assert_eq!(
        command,
        ship::key_of(
            &place,
            &deploy("vercel deploy --prod"),
            Some("work"),
            None,
            false
        )
        .0
    );
}

/// A workflow nobody could read was listed again with each run.
#[test]
fn an_unread_workflow_is_listed_once_however_often_it_runs() {
    let project = "/tmp/sikemux-gate-workflow";
    let core = core_at(None, project, Vec::new());
    for _ in 0..3 {
        let ships = crate::ship::of_command(
            "gh workflow run 4211 --repo acme/infra",
            &[],
            crate::ship::Looks::Every,
            Path::new("/nowhere"),
            &|_| None,
        );
        assert!(run(&core, ships[0].clone(), Path::new("/nowhere")).is_ok());
    }
    let read = of(&core, project);
    assert_eq!(read.seen.len(), 1);
    assert_eq!(read.seen[0].times, 3);
    assert!(crate::server::person_calls::shown(&core).is_empty());
}

/// Nightly tags went to the release workflow with no way to stop being asked each night.
#[tokio::test(flavor = "multi_thread")]
async fn a_nightly_asks_once_and_the_person_can_stop_it_asking() {
    let release = TAG_RELEASES;
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".github/workflows")).unwrap();
    std::fs::write(dir.path().join(".github/workflows/release.yml"), release).unwrap();
    let found = crate::server::places::found::in_files(dir.path());
    let names: Vec<&str> = found.iter().map(|place| place.name()).collect();
    assert!(names.contains(&"Nightly releases on GitHub"), "{names:?}");
    assert!(names.contains(&"Stable releases on GitHub"), "{names:?}");
    let project = "/tmp/sikemux-gate-nightly";
    let core = core_at(None, project, found);
    let tag = |name: &str| Ship {
        kind: ShipKind::Publish,
        what: format!("git push origin {name}"),
        target: Some(Target::Tag { name: name.into() }),
    };
    assert!(run(&core, tag("v0.6.2-nightly.1"), Path::new("/nowhere")).is_err());
    let call = open_calls(&core).remove(0);
    assert_eq!(
        call.place.as_ref().map(|place| place.name.as_str()),
        Some("Nightly releases on GitHub")
    );
    assert!(call
        .options
        .iter()
        .any(|option| option.id == ship::FROM_NOW_ON));
    answer(&core, &call.id, ship::FROM_NOW_ON).await;
    let went = run(&core, tag("v0.6.2-nightly.2"), Path::new("/nowhere")).unwrap();
    assert!(went.is_some());
    assert!(run(&core, tag("v0.6.2"), Path::new("/nowhere")).is_err());
    let stable = open_calls(&core).remove(0);
    assert_eq!(
        stable.place.as_ref().map(|place| place.name.as_str()),
        Some("Stable releases on GitHub")
    );
    assert!(!stable
        .options
        .iter()
        .any(|option| option.id == ship::FROM_NOW_ON));
}

/// A clean, pushed default branch fingerprinted the same at every commit.
#[test]
fn each_commit_of_a_clean_default_branch_is_new_work() {
    let dir = repo();
    let work = dir.path().join("work");
    git(&work, &["checkout", "main"]);
    let first = work::of(&work).unwrap();
    std::fs::write(work.join("app.txt"), "three\n").unwrap();
    git(&work, &["commit", "-qam", "three"]);
    git(&work, &["push", "-q", "origin", "main"]);
    assert_ne!(work::of(&work).unwrap(), first);
    let empty = tempfile::tempdir().unwrap();
    git(empty.path(), &["init", "-b", "main"]);
    assert_eq!(work::of(empty.path()), None);
}

/// A joiner whose turn ended first used up the yes for the asker, and one nobody held after a
/// restart never ended.
#[tokio::test(flavor = "multi_thread")]
async fn a_deploys_yes_ends_with_the_last_turn_of_those_on_its_call() {
    let dir = repo();
    let (cwd, project) = work_of(&dir);
    let core = core_at(None, &project, vec![vercel_place()]);
    let other = crate::server::names::Adopted {
        agent_id: "other",
        name: "Bo",
        project: project.clone(),
        parent: None,
        provider: "claude",
        reports: false,
        task: None,
    };
    core.names()
        .adopt(other, ["agent".to_owned(), "other".to_owned()].into());
    assert!(run(&core, deploy("vercel --prod"), &cwd).is_err());
    assert!(ship::check(&core, "other", deploy("vercel --prod"), &cwd).is_err());
    let call = open_calls(&core).remove(0);
    answer(&core, &call.id, "allow").await;
    core.ship().turn_started("agent");
    core.ship().turn_started("other");
    core.ship().turn_ended("other");
    assert!(run(&core, deploy("vercel --prod"), &cwd).unwrap().is_some());
    core.ship().turn_ended("agent");
    assert!(run(&core, deploy("vercel --prod"), &cwd).is_err());
}

/// "From now on just tell me" still asked while the list was unconfirmed.
#[tokio::test(flavor = "multi_thread")]
async fn from_now_on_counts_as_the_person_confirming_that_place() {
    let release = TAG_RELEASES;
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".github/workflows")).unwrap();
    std::fs::write(dir.path().join(".github/workflows/release.yml"), release).unwrap();
    let project = "/tmp/sikemux-gate-unconfirmed";
    let core = core_at(
        None,
        project,
        crate::server::places::found::in_files(dir.path()),
    );
    change(&core, project, |kept| {
        kept.confirmed = None;
        Ok(())
    })
    .unwrap();
    let tag = |name: &str| Ship {
        kind: ShipKind::Publish,
        what: format!("git push origin {name}"),
        target: Some(Target::Tag { name: name.into() }),
    };
    assert!(run(&core, tag("v0.6.2-nightly.1"), Path::new("/nowhere")).is_err());
    let call = open_calls(&core).remove(0);
    answer(&core, &call.id, ship::FROM_NOW_ON).await;
    assert!(run(&core, tag("v0.6.2-nightly.2"), Path::new("/nowhere")).is_ok());
    assert!(run(&core, tag("v0.6.2"), Path::new("/nowhere")).is_err());
}

/// The person said no to `git push origin v0.6.3`; the release was recut and the same push
/// was refused with no new call, and a later yes at Releases on GitHub never reached it.
#[tokio::test(flavor = "multi_thread")]
async fn a_no_to_a_tag_ends_when_the_tag_moves_or_the_place_gets_a_yes() {
    let dir = repo();
    let (cwd, _) = work_of(&dir);
    let cwd = sikemux_process::files::real_path(cwd).unwrap();
    let project = cwd.to_string_lossy().into_owned();
    let found = {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".github/workflows")).unwrap();
        std::fs::write(
            root.path().join(".github/workflows/release.yml"),
            TAG_RELEASES,
        )
        .unwrap();
        crate::server::places::found::in_files(root.path())
    };
    let core = core_at(None, &project, found);
    let push = |what: &str| Ship {
        kind: ShipKind::Push,
        what: what.into(),
        target: None,
    };
    let refused = |said: Result<Option<Deploy>, String>| {
        said.err().is_some_and(|said| said.starts_with("Not yet"))
    };
    git(&cwd, &["tag", "v0.6.3"]);
    assert!(run(&core, push("git push origin v0.6.3"), &cwd).is_err());
    let call = open_calls(&core).remove(0);
    answer(&core, &call.id, "refuse").await;
    assert!(refused(run(&core, push("git push origin v0.6.3"), &cwd)));
    assert!(open_calls(&core).is_empty());

    std::fs::write(cwd.join("app.txt"), "three\n").unwrap();
    git(&cwd, &["commit", "-qam", "three"]);
    git(&cwd, &["tag", "-f", "v0.6.3"]);
    assert!(!refused(run(&core, push("git push origin v0.6.3"), &cwd)));
    let call = open_calls(&core).remove(0);
    answer(&core, &call.id, "refuse").await;
    assert!(refused(run(&core, push("git push origin v0.6.3"), &cwd)));

    std::thread::sleep(std::time::Duration::from_millis(2));
    git(&cwd, &["tag", "v0.6.3-again"]);
    assert!(run(&core, push("git push origin v0.6.3-again"), &cwd).is_err());
    let call = open_calls(&core).remove(0);
    answer(&core, &call.id, "allow").await;
    assert!(!refused(run(&core, push("git push origin v0.6.3"), &cwd)));
    assert_eq!(open_calls(&core).len(), 1);
}

/// `git push origin main marker` with no release pattern for `marker` went past the person's
/// landing rule.
#[tokio::test(flavor = "multi_thread")]
async fn a_tag_pushed_with_the_branch_still_meets_the_landing_rule() {
    let dir = repo();
    let (cwd, _) = work_of(&dir);
    let cwd = sikemux_process::files::real_path(cwd).unwrap();
    let project = cwd.to_string_lossy().into_owned();
    git(&cwd, &["tag", "marker"]);
    let release = TAG_RELEASES;
    let found = {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".github/workflows")).unwrap();
        std::fs::write(root.path().join(".github/workflows/release.yml"), release).unwrap();
        crate::server::places::found::in_files(root.path())
    };
    let core = core_at(None, &project, found);
    let params = crate::server::board::BoardSettingsParams {
        project: project.clone(),
        landing: Some(crate::server::board::Landing::Person),
        ..Default::default()
    };
    crate::server::board::person_settings(&core, params)
        .await
        .unwrap();
    let push = Ship {
        kind: ShipKind::Push,
        what: "git push origin main marker".into(),
        target: None,
    };
    let said = run(&core, push, &cwd);
    assert!(
        said.as_ref().is_err_and(|said| said.starts_with("Held")),
        "{:?}",
        said.map(|deploy| deploy.is_some())
    );
}

/// An agent pushed its clone of another project's list with `git -C`, and was asked whether
/// to change its own project's stable releases, with its own project's latest commit shown.
#[tokio::test(flavor = "multi_thread")]
async fn a_push_from_another_repository_reaches_none_of_the_projects_places() {
    let (ours, theirs) = (repo(), repo());
    let (cwd, _) = work_of(&ours);
    let project = sikemux_process::files::real_path(&cwd).unwrap();
    let project = project.to_string_lossy().into_owned();
    let release = TAG_RELEASES;
    let found = {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".github/workflows")).unwrap();
        std::fs::write(root.path().join(".github/workflows/release.yml"), release).unwrap();
        crate::server::places::found::in_files(root.path())
    };
    let core = core_at(None, &project, found);
    let clone = theirs.path().join("work");
    let push = Ship {
        kind: ShipKind::Push,
        what: format!("git -C {} push -c x=y origin --tags", clone.display()),
        target: None,
    };
    assert!(run(&core, push, &cwd).is_ok_and(|deploy| deploy.is_none()));
    assert!(open_calls(&core).is_empty());
}

/// An agent in Sikemux's project ran `git -C ~/projects/personal/sikemux-front push origin main`,
/// which deploys sikemux.com on Vercel, and it went with nobody asked; so did a plain
/// `git push origin main` from an agent working in sikemux-front itself.
#[tokio::test(flavor = "multi_thread")]
async fn a_push_that_deploys_another_repositorys_vercel_is_held() {
    let (ours, theirs) = (repo(), repo());
    let (cwd, project) = work_of(&ours);
    let core = core_at(None, &project, Vec::new());
    let site = theirs.path().join("work");
    std::fs::write(
        site.join("vercel.json"),
        r#"{"git": {"deploymentEnabled": {"*": false, "main": true}}}"#,
    )
    .unwrap();
    let push = |what: &str| Ship {
        kind: ShipKind::Push,
        what: format!("git -C {} push origin {what}", site.display()),
        target: None,
    };
    assert!(run(&core, push("work"), &cwd).is_ok_and(|deploy| deploy.is_none()));
    assert!(open_calls(&core).is_empty());
    assert!(run(&core, push("main"), &cwd).is_err());
    assert_eq!(open_calls(&core).len(), 1);
    let (site, project) = work_of(&theirs);
    let found = crate::server::places::found::in_files(&site);
    let core = core_at(None, &project, found);
    let push = Ship {
        kind: ShipKind::Push,
        what: "git push origin main".to_owned(),
        target: None,
    };
    assert!(run(&core, push, &site).is_err());
    assert_eq!(open_calls(&core).len(), 1);
}

/// An agent pushed a scratch clone to its local bare remote with
/// `S=…; cd $S/r/w && /usr/bin/git push …` and was asked whether to change Sikemux's releases
/// on GitHub, shown the push without its `cd`.
#[tokio::test(flavor = "multi_thread")]
async fn a_push_after_a_change_of_folder_is_judged_where_it_runs() {
    let (ours, theirs) = (repo(), repo());
    let (cwd, project) = work_of(&ours);
    let found = {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".github/workflows")).unwrap();
        std::fs::write(
            root.path().join(".github/workflows/release.yml"),
            TAG_RELEASES,
        )
        .unwrap();
        crate::server::places::found::in_files(root.path())
    };
    let core = core_at(None, &project, found);
    let scratch = theirs.path().display();
    let ships =
        |line: &str| crate::ship::of_command(line, &[], crate::ship::Looks::Every, &cwd, &|_| None);
    let elsewhere = [
        format!("S={scratch}; cd $S/work && /usr/bin/git push --quiet origin HEAD:refs/heads/main"),
        format!("cd {scratch} && cd work && git push origin --tags"),
        format!("pushd {scratch}/work && git push origin --tags && popd"),
        format!("(cd {scratch}/work && git push origin --tags)"),
        format!("env -C {scratch}/work git push origin --tags"),
        format!("bash -c 'cd {scratch}/work && git push origin --tags'"),
    ];
    for line in &elsewhere {
        let found = ships(line);
        assert_eq!(found.len(), 1, "{line}");
        assert!(
            found[0].what.contains(&format!("{scratch}/work")),
            "{line}: {}",
            found[0].what
        );
        let ship = found.into_iter().next().unwrap();
        assert!(
            run(&core, ship, &cwd).is_ok_and(|deploy| deploy.is_none()),
            "{line}"
        );
    }
    assert!(open_calls(&core).is_empty());
    let here_again = format!("(cd {scratch}/work) && git push origin --tags");
    let ship = ships(&here_again).into_iter().next().unwrap();
    assert!(run(&core, ship, &cwd).is_err(), "{here_again}");
}

/// One stray untracked file in a clean, pushed checkout of main gave every commit the same
/// fingerprint.
#[test]
fn a_stray_file_beside_a_clean_default_branch_still_tells_commits_apart() {
    let dir = repo();
    let work = dir.path().join("work");
    git(&work, &["checkout", "main"]);
    std::fs::write(work.join("notes.txt"), "scratch").unwrap();
    let first = work::of(&work).unwrap();
    std::fs::write(work.join("app.txt"), "three\n").unwrap();
    git(&work, &["commit", "-qam", "three"]);
    git(&work, &["push", "-q", "origin", "main"]);
    assert_ne!(work::of(&work).unwrap(), first);
}

/// An asker that closed never took the turn that ends its yes, so the yes stayed open.
#[tokio::test(flavor = "multi_thread")]
async fn a_closed_askers_turn_counts_as_ended() {
    let dir = repo();
    let (cwd, project) = work_of(&dir);
    let core = core_at(None, &project, vec![vercel_place()]);
    assert!(ship::check(&core, "gone", deploy("vercel --prod"), &cwd).is_err());
    let call = open_calls(&core).remove(0);
    answer(&core, &call.id, "allow").await;
    assert!(run(&core, deploy("vercel --prod"), &cwd).is_err());
}

/// Two folders each running `--kubeconfig kubeconfig.yaml` with a context called default gave
/// one fact, so freeing dev freed prod.
#[test]
fn a_relative_kube_config_is_read_where_it_runs() {
    let clusters = tempfile::tempdir().unwrap();
    for name in ["dev", "other"] {
        let folder = clusters.path().join(name);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("kubeconfig.yaml"), "current-context: default\n").unwrap();
    }
    let read = |name: &str| {
        let line = "kubectl --kubeconfig kubeconfig.yaml apply -f k8s";
        let cwd = clusters.path().join(name);
        let ship = crate::ship::of_command(line, &[], crate::ship::Looks::Every, &cwd, &|_| None)
            .remove(0);
        ship.target
            .as_ref()
            .and_then(Target::read_at)
            .map(str::to_owned)
    };
    assert_ne!(read("dev"), read("other"));
    let both = [("AWS_PROFILE", "dev"), ("AWS_DEFAULT_PROFILE", "prod")];
    let env = |name: &str| {
        (both.iter())
            .find(|(key, _)| *key == name)
            .map(|(_, value)| (*value).to_owned())
    };
    let ships = crate::ship::of_command(
        "aws ecs update-service --service web",
        &[],
        crate::ship::Looks::Every,
        Path::new("/nowhere"),
        &env,
    );
    assert_eq!(ships[0].target, None);
}

fn closed_after_restart(data: &Path, id: &str) -> Option<CallResult> {
    let again = Core::new(Default::default(), Some(data)).unwrap();
    let call = crate::server::person_calls::shown(&again)
        .into_iter()
        .find(|call| call.id == id)?;
    call.outcome.map(|outcome| outcome.result)
}

async fn answer_on_the_card(core: &Arc<Core>, id: &str) {
    allow_on_the_card(core, id, true).await.unwrap();
}

/// Taps allow on the call's card, with the core's own window answering `window_says`.
async fn allow_on_the_card(core: &Arc<Core>, id: &str, window_says: bool) -> CoreResult<()> {
    core.remote().answer_window_with(window_says);
    let seen = sikemux_view::calls::CallParams { id: id.to_owned() };
    crate::server::person_calls::seen(core, &Peer::App, seen).unwrap();
    let params = sikemux_view::calls::AnswerCallParams {
        id: id.to_owned(),
        option: Some("allow".to_owned()),
        options: Vec::new(),
        note: None,
        words: None,
        fine: false,
        paths: Vec::new(),
        none: false,
    };
    crate::server::person_calls::answer(core, &Peer::App, params).await
}

/// A no in the core's own window refuses the yes tapped on the card.
#[tokio::test(flavor = "multi_thread")]
async fn a_no_in_the_window_refuses_the_card() {
    let dir = repo();
    let data = tempfile::tempdir().unwrap();
    let (cwd, project) = work_of(&dir);
    let core = core_at(Some(data.path()), &project, vec![vercel_place()]);
    assert!(run(&core, deploy("vercel --prod"), &cwd).is_err());
    let id = open_calls(&core).remove(0).id;
    assert!(allow_on_the_card(&core, &id, false).await.is_err());
    assert_eq!(open_calls(&core).remove(0).id, id);
    assert!(run(&core, deploy("vercel --prod"), &cwd).is_err());
}

/// A tap in the app marked the gate's call started before the agent ran anything, so a restart
/// then said it may have run.
#[tokio::test(flavor = "multi_thread")]
async fn a_tap_on_the_card_starts_nothing_until_the_agent_runs_it() {
    let dir = repo();
    let data = tempfile::tempdir().unwrap();
    let (cwd, project) = work_of(&dir);
    let core = core_at(Some(data.path()), &project, vec![vercel_place()]);
    assert!(run(&core, deploy("vercel --prod"), &cwd).is_err());
    let id = open_calls(&core).remove(0).id;
    answer_on_the_card(&core, &id).await;
    drop(core);
    assert_eq!(
        closed_after_restart(data.path(), &id),
        Some(CallResult::NotShipped)
    );
}

/// A deploy's retry after its first command ended kept the first command's result through a
/// restart, and the agent heard nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_retry_that_started_after_an_end_may_have_run() {
    let dir = repo();
    let data = tempfile::tempdir().unwrap();
    let (cwd, project) = work_of(&dir);
    let core = core_at(Some(data.path()), &project, vec![vercel_place()]);
    assert!(run(&core, deploy("vercel --prod"), &cwd).is_err());
    let id = open_calls(&core).remove(0).id;
    answer_on_the_card(&core, &id).await;
    let first = run(&core, deploy("vercel --prod"), &cwd).unwrap().unwrap();
    ship::ended(core.clone(), first, Some(Verdict::Failed)).await;
    std::thread::sleep(std::time::Duration::from_millis(2));
    assert!(run(&core, deploy("vercel --prod"), &cwd).unwrap().is_some());
    drop(core);
    assert_eq!(
        closed_after_restart(data.path(), &id),
        Some(CallResult::MayHaveRun)
    );
}

/// @Wataru widened a tag ruleset on nodelike/sikemux, the project's own repository, and the
/// person got "Set rulesets/24037079 on nodelike/sikemux?" from a place an older gate had kept
/// for every `gh api` write.
#[test]
fn a_github_write_to_the_projects_own_repository_runs_and_others_ask_in_words() {
    let dir = repo();
    let (cwd, project) = work_of(&dir);
    let url = "https://github.com/nodelike/sikemux.git";
    git(&cwd, &["remote", "set-url", "origin", url]);
    let kept = new_place(
        "writes on GitHub's API",
        true,
        vec![Recognise::Stage {
            tool: "GitHub's API".into(),
            name: "writes".into(),
        }],
        None,
    );
    let core = core_at(None, &project, vec![kept]);
    let ship = |line: &str| {
        let looks = crate::ship::Looks::Every;
        crate::ship::of_command(line, &[], looks, &cwd, &|_| None).remove(0)
    };
    let own = "gh api -X PUT repos/nodelike/sikemux/rulesets/24037079 --input - --jq .conditions";
    assert!(matches!(run(&core, ship(own), &cwd), Ok(None)));
    let filled = "gh api -X POST repos/{owner}/{repo}/labels -f name=phone";
    assert!(matches!(run(&core, ship(filled), &cwd), Ok(None)));
    assert!(open_calls(&core).is_empty());

    let theirs = "gh api -X PUT repos/acme/web/rulesets/24037079 --input -";
    assert!(run(&core, ship(theirs), &cwd).is_err());
    let call = open_calls(&core).remove(0);
    assert_eq!(call.question, "Change who may push what to acme/web?");
}
