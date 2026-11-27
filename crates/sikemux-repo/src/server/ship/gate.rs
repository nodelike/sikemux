use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::calls::{
    CallAsker, CallKind, CallOption, CallPlace, CallResult, CallWhy, PersonCall,
};
use sikemux_view::inbox::ShipKind;
use sikemux_view::places::Recognise;

use super::{hold::tell_calls, Attempt, Deploy, Hold, MAX_HOLDS_PER_AGENT};
use crate::git::Git;
use crate::server::agent_now::AgentNowCore;
use crate::server::changed::{ChangedCore, What};
use crate::server::credential;
use crate::server::person_calls::{self, Raise, TakenBack};
use crate::server::places::{self, push};
use crate::server::repo_state::RepoParts;
use crate::server::workspace_state::WorkspaceParts;
use crate::server::Core;
use crate::ship::{Ship, Target};

/// What a yes at `place` is kept under for `ship`, and whether it covers the action once. A
/// deploy of work with a fingerprint is covered with its retries, however spelled, at the same
/// target, but one sent by a push, or naming what it changes, apart from other deploys.
/// Anything else, or work git can't fingerprint, is covered only as the exact action sending
/// the same commits `sends`, such as those a pushed tag names, from any checkout of the project.
pub fn key_of(
    place: &places::Place,
    ship: &Ship,
    work: Option<&str>,
    sends: Option<&str>,
    tool: bool,
) -> (String, bool) {
    let target = (ship.target.as_ref())
        .and_then(|target| serde_json::to_string(target).ok())
        .unwrap_or_default();
    let digest = |text: &str| {
        use sha2::Digest;
        let hash = sha2::Sha256::digest(text.as_bytes());
        hex::encode(hash).get(..16).unwrap_or_default().to_owned()
    };
    let id = &place.shown.id;
    match (ship.kind, work) {
        (ShipKind::Deploy, Some(work)) if !tool => (
            format!(
                "place:{id}:work:{work}:to:{}",
                match sends {
                    Some(sends) => digest(&format!("{target}\n{sends}")),
                    None => digest(&target),
                }
            ),
            false,
        ),
        _ => {
            let sends = sends.unwrap_or_default();
            let action = digest(&format!("{target}\n{}\n{sends}", ship.what));
            (format!("place:{id}:work::action:{action}"), true)
        }
    }
}

/// Whether the Vercel command `what` changes what it names, such as an older deployment or a
/// domain, rather than deploying the work in the checkout.
pub(super) fn names_its_own(what: &str) -> bool {
    let words: Vec<&str> = what.split_whitespace().collect();
    let named = [
        "promote", "rollback", "redeploy", "alias", "rm", "remove", "--cwd", "--scope", "-S",
        "--team", "-T", "--token", "-t",
    ];
    let is_vercel = |word: &str| {
        let name = word
            .rsplit('/')
            .next()
            .and_then(|name| name.split('@').next());
        matches!(name, Some("vercel" | "vc"))
    };
    words.iter().any(|word| is_vercel(word))
        && (words.iter().any(|word| {
            let word = word.split('=').next().unwrap_or(word);
            named.contains(&word)
        }) || crate::ship::vercel_takes_away(&words))
}

/// Whether an answered call is a yes that still stands: picked as its allow option since this
/// core started, not yet used, and its deploy not yet ended well.
pub(super) fn passes(core: &Core, call: &PersonCall, asking: &str) -> bool {
    settle_covering(core, &call.id, asking);
    let yes = (call.answer.as_ref())
        .is_some_and(|answer| said_yes(answer.option.as_deref()) && answer.at >= core.ship().born);
    if core.ship().state.locked().used.contains(&call.id) {
        return false;
    }
    let used = (call.outcome.as_ref()).is_some_and(|outcome| {
        matches!(
            outcome.result,
            CallResult::NotShipped | CallResult::Withdrawn
        )
    });
    yes && !used
}

/// A deploy's yes covers those on its call until the last of their turns ends; an agent that
/// closed takes no more turns, so it counts as ended.
fn settle_covering(core: &Core, call: &str, asking: &str) {
    let on_call: Vec<String> = (core.ship().state.locked().covering.iter())
        .filter(|(_, covered, _)| covered == call)
        .map(|(agent, _, _)| agent.clone())
        .collect();
    if on_call.is_empty() {
        return;
    }
    let gone: Vec<String> = (on_call.into_iter())
        .filter(|agent| agent != asking)
        .filter(|agent| !core.agent_now(agent).running() && core.workspaces().closed(agent))
        .collect();
    if gone.is_empty() {
        return;
    }
    let mut state = core.ship().state.locked();
    state
        .covering
        .retain(|(agent, covered, _)| covered != call || !gone.contains(agent));
    if !state.covering.iter().any(|(_, covered, _)| covered == call) {
        state.used.insert(call.to_owned());
    }
}

fn refused(call: &PersonCall) -> bool {
    (call.answer.as_ref()).is_some_and(|answer| !said_yes(answer.option.as_deref()))
}

/// The option of a routine change's call that says yes, and stops asking for such changes.
pub const FROM_NOW_ON: &str = "allowFromNowOn";

fn said_yes(option: Option<&str>) -> bool {
    matches!(option, Some(person_calls::HOLD_ALLOW | FROM_NOW_ON))
}

/// What a gate asks the person about: the place, the key its answer is kept under, and whether
/// a yes covers the action once. Only a routine change offers to stop asking, for the repository
/// it goes through.
pub(super) struct Asking<'a> {
    pub(super) place: &'a places::Place,
    pub(super) key: String,
    pub(super) once: bool,
    pub(super) routine: Option<String>,
    pub(super) attempt: Option<Attempt>,
}

/// The call that puts a change of `ship` to the production place `place` to the person, which
/// offers to stop asking for a routine one. With nothing to ask, it only tells.
pub(super) fn call_for(
    core: &Core,
    agent_id: &str,
    project: &str,
    ship: &Ship,
    cwd: &Path,
    place: &places::Place,
    asking: Option<&Asking<'_>>,
) -> Raise {
    let name = core.names().name_or_id(agent_id);
    let subject = (ship.kind == ShipKind::Deploy)
        .then(|| Git::quick(cwd).trimmed(&["log", "-1", "--format=%s"]).ok())
        .flatten()
        .filter(|subject| !subject.is_empty());
    let here = || {
        let remote = Git::quick(cwd)
            .trimmed(&["remote", "get-url", "origin"])
            .ok()?;
        let (owner, name) = credential::owner_and_name(&remote)?;
        Some(format!("{owner}/{name}"))
    };
    let is_git_push = ship
        .what
        .split_whitespace()
        .next()
        .is_some_and(|program| program.rsplit('/').next() == Some("git"));
    let tag = match (&ship.target, push::deletes(&ship.what)) {
        (Some(Target::Tag { .. }), Some(deleted)) if is_git_push => {
            let repo = here().unwrap_or_else(|| "GitHub".to_owned());
            let counted = counted(&deleted);
            let named = match deleted.len() > NAMED_AT_MOST {
                true => counted.clone(),
                false => deleted.join(", "),
            };
            Some((
                format!("Delete {named} on {repo}"),
                format!("Delete {counted}"),
            ))
        }
        (Some(Target::Tag { name }), None) if is_git_push && !name.is_empty() => Some((
            format!("Publish {name} to {}", place.name()),
            "Publish the tag".to_owned(),
        )),
        (Some(Target::Tag { .. }), None) if is_git_push => Some((
            match push::named_refs(&ship.what) {
                refs if refs > 1 => format!(
                    "Run this push of {refs} refs, which may send tags to {}",
                    place.name()
                ),
                _ => format!("Run this push, which may send tags to {}", place.name()),
            },
            "Run this push".to_owned(),
        )),
        _ => None,
    };
    let write = tag
        .or_else(|| github_release_write(&ship.what, place.name()))
        .or_else(|| github_write(&ship.what, here).map(|said| (said.clone(), said)));
    let option = |id: &str, label: String| CallOption {
        id: id.to_owned(),
        label,
        detail: None,
        picture: None,
    };
    Raise {
        project: project.to_owned(),
        kind: CallKind::Production,
        asker: CallAsker {
            agent_id: agent_id.to_owned(),
            cli: (core.repo_above().questions_cli_of)(core, agent_id),
            name,
            chief: false,
        },
        question: (write.as_ref().map(|(write, _)| format!("{write}?")))
            .unwrap_or_else(|| format!("Change {} with this work?", place.name())),
        context: subject.map(|subject| format!("The work's latest commit: {subject}")),
        options: [
            Some(option(
                person_calls::HOLD_ALLOW,
                write.map_or_else(|| format!("Change {}", place.name()), |(_, button)| button),
            )),
            asking
                .and_then(|asking| asking.routine.as_ref())
                .map(|repository| CallOption {
                    detail: Some(format!(
                        "For plain pushes and merges to {repository}. Anything else there still waits for you."
                    )),
                    ..option(
                        FROM_NOW_ON,
                        format!("Change {}, and from now on just tell me", place.name()),
                    )
                }),
            Some(option(person_calls::HOLD_REFUSE, "Not now".to_owned())),
        ]
        .into_iter()
        .flatten()
        .collect(),
        pick: None,
        against: None,
        evidence: Vec::new(),
        action: Some(ship.what.clone()),
        brief: None,
        place: Some(CallPlace {
            id: place.shown.id.clone(),
            name: place.name().to_owned(),
            repository: asking.and_then(|asking| asking.routine.clone()),
        }),
        key: asking.map(|asking| asking.key.clone()),
        yes: Some(person_calls::HOLD_ALLOW.to_owned()),
        attempt: asking.and_then(|asking| asking.attempt.clone().map(Box::new)),
    }
}

/// The most refs a call names; it counts any more.
const NAMED_AT_MOST: usize = 3;

/// The refs `deleted`, as [`push::deletes`] names them, in a few words: "the tag", "107 refs".
fn counted(deleted: &[String]) -> String {
    let all = |kind: &str| deleted.iter().all(|name| name.starts_with(kind));
    let (one, many) = match (all("tag "), all("branch ")) {
        (true, _) => ("tag", "tags"),
        (_, true) => ("branch", "branches"),
        _ => ("ref", "refs"),
    };
    match deleted.len() {
        1 => format!("the {one}"),
        count => format!("{count} {many}"),
    }
}

/// What a `gh api` command writes on GitHub, in the words the person would use for it, such
/// as "Comment on owner/repo#12", and never an endpoint or an id. `here` names the checkout's
/// repository, which gh puts in for `{owner}/{repo}`.
pub(super) fn github_write(what: &str, here: impl FnOnce() -> Option<String>) -> Option<String> {
    let command = what.rsplit(" && ").next()?;
    let words: Vec<&str> = command.split_whitespace().collect();
    let ["gh", "api", args @ ..] = words.as_slice() else {
        return None;
    };
    let call = crate::ship::github_api(args)?;
    let Some(named) = call.repo() else {
        return Some("Write to GitHub as you".to_owned());
    };
    let repo = match named.split_once('/') {
        Some(("{owner}", _) | (_, "{repo}")) => {
            here().unwrap_or_else(|| "this repository".to_owned())
        }
        _ => named,
    };
    let path = call.endpoint.split(['?', '#']).next().unwrap_or_default();
    let rest: Vec<&str> = path.split('/').skip(3).collect();
    let said = match (call.method.as_str(), rest.as_slice()) {
        ("POST", ["issues" | "pulls", number, "comments", ..]) => {
            format!("Comment on {repo}#{number}")
        }
        ("POST", ["pulls", number, "reviews", ..]) => format!("Review {repo}#{number}"),
        ("PATCH", ["issues" | "pulls", number]) => format!("Edit {repo}#{number}"),
        ("PUT", ["pulls", number, "merge"]) => format!("Merge {repo}#{number}"),
        ("POST", ["issues"]) => format!("Open an issue on {repo}"),
        ("POST", ["pulls"]) => format!("Open a pull request on {repo}"),
        ("POST", ["forks"]) => format!("Fork {repo}"),
        ("PUT", ["contents", path @ ..]) => format!("Update {} in {repo}", path.join("/")),
        ("DELETE", ["contents", path @ ..]) => format!("Delete {} in {repo}", path.join("/")),
        ("POST", ["git", "refs"]) => format!("Create a branch in {repo}"),
        ("PATCH", ["git", "refs", "heads", branch @ ..]) => {
            format!("Move branch {} in {repo}", branch.join("/"))
        }
        ("DELETE", ["git", "refs", "heads", branch @ ..]) => {
            format!("Delete branch {} in {repo}", branch.join("/"))
        }
        ("POST", ["releases"]) => format!("Publish a release of {repo}"),
        ("PATCH", ["releases", _]) => format!("Edit a release of {repo}"),
        ("DELETE", ["releases", _]) => format!("Delete a release of {repo}"),
        ("POST", ["releases", _, "assets"]) => format!("Upload a release asset to {repo}"),
        ("PATCH", ["releases", "assets", _]) => format!("Edit a release asset of {repo}"),
        ("DELETE", ["releases", "assets", _]) => format!("Delete a release asset of {repo}"),
        ("DELETE", []) => format!("Delete {repo}"),
        ("PATCH", []) => format!("Change the settings of {repo}"),
        ("POST", ["pages"]) => format!("Turn on GitHub Pages for {repo}"),
        ("PUT", ["pages"]) => format!("Change GitHub Pages for {repo}"),
        ("DELETE", ["pages"]) => format!("Turn off GitHub Pages for {repo}"),
        ("POST", ["pages", "builds"]) => format!("Rebuild GitHub Pages for {repo}"),
        (_, ["rulesets", ..]) => format!("Change who may push what to {repo}"),
        (_, ["branches", _, "protection", ..]) => format!("Change branch protection on {repo}"),
        (_, ["labels", ..]) => format!("Change the labels of {repo}"),
        (_, ["collaborators", ..]) => format!("Change who can work on {repo}"),
        (_, ["hooks", ..]) => format!("Change the webhooks of {repo}"),
        (_, ["actions", "secrets", ..]) => format!("Change the secrets of {repo}"),
        (_, ["actions", ..] | ["environments", ..]) => {
            format!("Change how GitHub Actions runs for {repo}")
        }
        _ => format!("Change {repo} on GitHub as you"),
    };
    Some(said)
}

/// What a `gh release` command changes, in the words the person would use for it.
fn github_release_write(what: &str, place: &str) -> Option<(String, String)> {
    let command = what.rsplit(" && ").next()?;
    let words: Vec<&str> = command.split_whitespace().collect();
    let [gh, rest @ ..] = words.as_slice() else {
        return None;
    };
    if gh.rsplit('/').next() != Some("gh") {
        return None;
    }
    let at = rest.iter().position(|word| *word == "release")?;
    let sub = *rest.get(at + 1)?;
    let (question, button) = match sub {
        "create" => (
            format!("Publish a release to {place}"),
            "Publish the release",
        ),
        "edit" => (format!("Edit a release in {place}"), "Edit the release"),
        "upload" => (format!("Upload assets to {place}"), "Upload assets"),
        "delete" => (format!("Delete a release in {place}"), "Delete the release"),
        "delete-asset" => (
            format!("Delete a release asset in {place}"),
            "Delete the asset",
        ),
        _ => (format!("Change a release in {place}"), "Change the release"),
    };
    Some((question, button.to_owned()))
}

/// The target a change to the GitHub release `tag` is judged under in `project`: a tag place
/// that recognises `tag`, or GitHub releases as a whole when none does.
pub(super) fn release_target(core: &Core, project: &str, tag: &str) -> Target {
    let probe = Ship {
        kind: ShipKind::Publish,
        what: String::new(),
        target: Some(Target::Tag {
            name: tag.to_owned(),
        }),
    };
    let (places, _) = places::current(core, project);
    let matches_tag = places.iter().any(|place| {
        place.recognise.iter().any(|fact| {
            matches!(fact, Recognise::Tag { .. }) && places::place::covers(fact, &probe)
        })
    });
    match matches_tag {
        true => Target::Tag {
            name: tag.to_owned(),
        },
        false => Target::Package {
            registry: places::GITHUB_RELEASES.to_owned(),
        },
    }
}

/// Whether the project keeps a place for tags it pushes.
pub(super) fn tag_places(core: &Core, project: &str) -> bool {
    let (places, _) = places::current(core, project);
    (places.iter())
        .flat_map(|place| place.recognise.iter())
        .any(|fact| matches!(fact, Recognise::Tag { .. }))
}

/// Whether a workflow the project keeps publishes a release, so a pushed tag can start one.
pub(super) fn releases_on_tags(core: &Core, project: &str) -> bool {
    let releases = Recognise::Package {
        registry: places::GITHUB_RELEASES.to_owned(),
    };
    let (places, _) = places::current(core, project);
    places
        .iter()
        .any(|place| place.recognise.contains(&releases))
}

/// A change to the production place `place` waits for the person's yes on one call for this
/// piece of work there, which every agent waiting on the same work there joins. A no holds for
/// that work there until the work changes, the person writes to the agent again or says yes
/// to another change there, with no second call. An agent waits on one call at a place: a new action there, such as a retry
/// spelled another way, withdraws the call it waited on.
pub(super) fn gate(
    core: &Core,
    agent_id: &str,
    project: &str,
    ship: Ship,
    cwd: &Path,
    asking: Asking<'_>,
    why: Option<String>,
) -> Result<Option<Deploy>, String> {
    let Asking {
        place, key, once, ..
    } = &asking;
    let once = *once;
    if let Some(no) = person_calls::find_key(core, project, key).filter(refused) {
        let answered = no.answer.as_ref().map_or(0, |answer| answer.at);
        let spoke = core.ship().spoke_at(agent_id);
        let yes_since = person_calls::find_keys(core, project, |held| same_place(held, key))
            .into_iter()
            .filter_map(|(_, call)| call.answer)
            .any(|answer| said_yes(answer.option.as_deref()) && answer.at > answered);
        if spoke.is_none_or(|spoke| spoke <= answered) && !yes_since {
            return Err(format!(
                "Not yet: the person said no to changing {} with this work, until the work changes or they tell you otherwise.",
                place.name()
            ));
        }
    }
    let stale: Vec<String> = (core.ship().state.locked().holds.iter())
        .filter(|hold| hold.agent_id == agent_id)
        .filter_map(|hold| hold.key.as_deref().map(|held| (hold, held)))
        .filter(|(_, held)| *held != key && same_place(held, key))
        .map(|(hold, _)| hold.token.clone())
        .collect();
    for token in stale {
        withdraw(core, agent_id, &token, "its agent asked again there");
    }
    let oldest = {
        let state = core.ship().state.locked();
        let mine: Vec<&Hold> = (state.holds.iter())
            .filter(|hold| hold.agent_id == agent_id && hold.key.is_some())
            .collect();
        (mine.first())
            .filter(|_| mine.len() >= MAX_HOLDS_PER_AGENT)
            .map(|oldest| (oldest.token.clone(), oldest.ship.what.clone()))
    };
    if let Some((token, what)) = oldest {
        withdraw(
            core,
            agent_id,
            &token,
            "it waited on more calls than Sikemux keeps",
        );
        let text = format!(
            "Sikemux withdrew its call for `{what}`, the oldest of the calls you wait on, as you wait on more than it keeps. Run it again when you need it."
        );
        tell_soon(core, agent_id, text);
    }
    let raise = call_for(core, agent_id, project, &ship, cwd, place, Some(&asking));
    let id = match person_calls::raise(core, raise) {
        Ok(id) => id,
        Err(why) => {
            return Err(format!(
                "Waits for the person: `{}` changes {}, a production place, but Sikemux couldn't put the call to them ({why}). Don't run it; try again in a moment.",
                ship.what,
                place.name()
            ))
        }
    };
    {
        let mut state = core.ship().state.locked();

        let waiting = (state.holds.iter())
            .any(|hold| hold.agent_id == agent_id && hold.key.as_deref() == Some(key.as_str()));
        if !waiting {
            state.holds.push(Hold {
                token: id.clone(),
                agent_id: agent_id.to_owned(),
                project: project.to_owned(),
                cwd: cwd.to_owned(),
                at: epoch_ms(),
                environment: Some(place.name().to_owned()),
                why: why.clone(),
                because: None,
                deploy: None,
                call: None,
                key: Some(key.clone()),
                ship: ship.clone(),
            });
        }
    }
    core.changed(What::Agents);
    let covers = match once {
        true => "this action once",
        false => "this work's deploy there until the end of that turn",
    };
    let mut said = format!(
        "Waits for the person's yes: `{}` changes {}, a production place. You will hear their answer; a yes covers {covers}. Running anything else there withdraws this call, so put every change you need there in one command. Go on with other work meanwhile.",
        ship.what,
        place.name()
    );
    if let Some(why) = why {
        said = format!("{said} {why}");
    }
    let unexplained = person_calls::open_call(core, &id)
        .is_some_and(|call| call.asker.agent_id == agent_id && !explained(&call.question));
    if unexplained {
        said = format!("{said} {}", explain(&id));
    }
    Err(said)
}

/// What the agent waiting on the held call `id` is asked for, so the person reads its reason.
pub(super) fn explain(id: &str) -> String {
    format!("Now call ask with edit: \"{id}\" and, as question, one or two plain sentences for the person: why this is needed and what is lost if they say yes. If you stop needing it, take it back: the same edit with withdraw: true.")
}

/// A held call's question with its agent's reason under it, in place of any before.
pub(crate) fn with_reason(question: &str, reason: &str) -> String {
    let asked = question.lines().next().unwrap_or_default();
    format!("{asked}\n\n{reason}")
}

fn explained(question: &str) -> bool {
    question.contains('\n')
}

/// Puts `reason`, why `agent_id` needs what the call `id` holds, on that call. False when it
/// holds nothing of that agent's.
pub fn explain_held(core: &Core, agent_id: &str, id: &str, reason: &str) -> bool {
    let held = {
        let mut state = core.ship().state.locked();
        let hold = (state.holds.iter_mut())
            .find(|hold| hold.token == id && hold.agent_id == agent_id && hold.key.is_none());
        hold.map(|hold| hold.because = Some(reason.to_owned()))
            .is_some()
    };
    if held {
        tell_calls(core);
        return true;
    }
    person_calls::explain(core, agent_id, id, reason)
}

/// Sends the agent `text` as from Sikemux, without waiting on it.
fn tell_soon(core: &Core, agent_id: &str, text: String) {
    let (Some(core), agent_id) = (core.arc(), agent_id.to_owned()) else {
        return;
    };
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        runtime.spawn(async move {
            let _ = (core.repo_above().messages_for_person)(&core, agent_id, text).await;
        });
    }
}

/// Whether `dir` is in the repository of `project`, one of its worktrees or a clone that
/// pushes to one of its remotes. A push from another repository, such as a clone of someone
/// else's, reaches none of the project's places.
pub(super) fn same_repository(dir: &Path, project: &str) -> bool {
    let read = |at: &Path| {
        let git = || Git::quick(at);
        let common = git()
            .trimmed(&["rev-parse", "--path-format=absolute", "--git-common-dir"])
            .ok()?;
        let remotes = git().trimmed(&["remote", "-v"]).unwrap_or_default();
        let urls: HashSet<String> = (remotes.lines())
            .filter_map(|line| line.split_whitespace().nth(1).map(str::to_owned))
            .collect();
        Some((
            sikemux_process::files::real_path(PathBuf::from(common)).ok()?,
            urls,
        ))
    };
    match (read(dir), read(Path::new(project))) {
        (Some((pushed, theirs)), Some((own, ours))) => pushed == own || !theirs.is_disjoint(&ours),
        _ => true,
    }
}

/// The main checkout of the repository a push from `dir` sends, which can sit inside a
/// `project` that is only a folder holding repositories, such as the home folder.
pub(super) fn checkout_of(dir: &Path, project: &str) -> String {
    (crate::git::main_checkout(dir))
        .or_else(|| sikemux_process::files::repo_root(dir).map(Path::to_path_buf))
        .map_or_else(
            || project.to_owned(),
            |root| {
                let root = sikemux_process::files::real_path(&root).unwrap_or(root);
                root.to_string_lossy().into_owned()
            },
        )
}

/// Whether the GitHub repository `repo` belongs to whoever owns the remotes of the checkout at
/// `cwd` or of `project`, as gh's `{owner}` does, so a write there is the project's own work.
pub(super) fn owned_here(repo: &str, cwd: &Path, project: &str) -> bool {
    let owner = repo.split('/').next().unwrap_or_default();
    if owner.is_empty() || owner == "{owner}" {
        return !repo.is_empty();
    }
    [cwd, Path::new(project)].into_iter().any(|dir| {
        let remotes = Git::quick(dir)
            .trimmed(&["remote", "-v"])
            .unwrap_or_default();
        (remotes.lines())
            .filter_map(|line| line.split_whitespace().nth(1))
            .filter_map(credential::owner_and_name)
            .any(|(theirs, _)| theirs.eq_ignore_ascii_case(owner))
    })
}

/// Whether two keys name the same place, whatever the work or the action.
fn same_place(one: &str, other: &str) -> bool {
    let place = |key: &str| key.split_once(":work:").map(|(place, _)| place.to_owned());
    place(one).is_some_and(|one| Some(one) == place(other))
}

/// Drops `agent_id`'s wait on the gate call `token`, and withdraws the call when nobody else
/// waits on it.
fn withdraw(core: &Core, agent_id: &str, token: &str, why: &str) {
    if let TakenBack::Done = drop_hold(core, agent_id, token, None) {
        eprintln!("sikemux core: @{agent_id}'s wait on call {token} was withdrawn, as {why}");
    }
}

/// Takes back the production call `id` that `agent_id` waits on, with the action held for it,
/// so a later yes runs nothing. None when that agent waits on no such call.
pub fn take_back(core: &Core, agent_id: &str, id: &str) -> Option<Result<(), String>> {
    match drop_hold(core, agent_id, id, Some(CallWhy::TakenBack)) {
        TakenBack::Done => Some(Ok(())),
        TakenBack::TooLate => Some(Err(
            "that call was answered or closed already, so there is nothing left to take back"
                .to_owned(),
        )),
        TakenBack::NotTheirs => None,
    }
}

/// Takes back `agent_id`'s wait on the call `id` and drops what is held for it, while the call
/// still waits on the person.
fn drop_hold(core: &Core, agent_id: &str, id: &str, why: Option<CallWhy>) -> TakenBack {
    let mut state = core.ship().state.locked();
    let gone = |agent: &str| core.workspaces().closed(agent);
    let taken = person_calls::take_back(core, agent_id, id, why, gone);
    if let TakenBack::Done = taken {
        (state.holds).retain(|hold| !(hold.token == id && hold.agent_id == agent_id));
        drop(state);
        core.changed(What::Agents);
    }
    taken
}

/// The person answered the call a production gate raised: each agent waiting on it hears
/// so, and what waits for it is no longer held. A no holds for those agents until the end of
/// the turn in which they hear it.
pub async fn gate_answered(core: &Arc<Core>, call: &PersonCall, typed: Option<String>) {
    let option = call
        .answer
        .as_ref()
        .and_then(|answer| answer.option.clone());
    let once = person_calls::key_of(core, &call.id).is_some_and(|key| key.contains(":action:"));
    {
        let mut state = core.ship().state.locked();
        let mut waiting: Vec<String> = (state.holds.iter())
            .filter(|hold| hold.token == call.id)
            .map(|hold| hold.agent_id.clone())
            .collect();
        waiting.extend(
            std::iter::once(&call.asker)
                .chain(&call.joined)
                .map(|asker| asker.agent_id.clone()),
        );
        waiting.sort();
        waiting.dedup();
        state.holds.retain(|hold| hold.token != call.id);
        if said_yes(option.as_deref()) && !once {
            for agent_id in waiting {
                state.covering.push((agent_id, call.id.clone(), false));
            }
        }
    }
    core.changed(What::Agents);
    let from = call.answer.as_ref().map(|answer| match answer.by {
        sikemux_view::calls::CallClient::Device { .. } => "a paired phone".to_owned(),
        _ => "Sikemux".to_owned(),
    });
    if option.as_deref() == Some(FROM_NOW_ON) {
        let repository = (call.place.as_ref()).and_then(|place| place.repository.as_deref());
        if let (Some(place), Some(repository)) = (place_of(core, call), repository) {
            places::just_tell(core, &call.project, place, repository, from);
        }
    }
    let asked = call.question.lines().next().unwrap_or_default();
    let place = (call.place.as_ref()).map_or("the place", |place| place.name.as_str());
    let action = call.action.as_deref().unwrap_or("it");
    let mut text = match option.as_deref() {
        Some(FROM_NOW_ON) => format!(
            "The person said yes on the call \"{}\", and to just tell them from now on. Run `{action}` again now.",
            asked
        ),
        _ if said_yes(option.as_deref()) && once => format!(
            "The person said yes on the call \"{}\". Run exactly `{action}` again now. The yes lets it run once, and stands until it runs without failing.",
            asked
        ),
        _ if said_yes(option.as_deref()) => format!(
            "The person said yes on the call \"{}\". Run `{action}` again now; the yes covers this work's deploy to {place} until the end of this turn.",
            asked
        ),
        _ => format!(
            "The person said no on the call \"{}\": {place} stays as it is with this work.",
            asked
        ),
    };
    if let Some(typed) = typed {
        text = format!("{text}\nThey added: {typed}");
    }
    let askers = std::iter::once(&call.asker).chain(&call.joined);
    let mut told: Vec<&str> = Vec::new();
    for asker in askers {
        if told.contains(&asker.agent_id.as_str()) {
            continue;
        }
        told.push(&asker.agent_id);
        let delivered =
            (core.repo_above().messages_for_person)(core, asker.agent_id.clone(), text.clone())
                .await;
        let heard = delivered.is_ok_and(|told| {
            matches!(
                told.get("delivery").and_then(Value::as_str),
                Some("steered" | "newTurn")
            )
        });
        if heard {
            core.ship().turn_started(&asker.agent_id);
        }
    }
}

/// The place a call was raised at, as the project keeps it.
fn place_of(core: &Core, call: &PersonCall) -> Option<places::Place> {
    let shown = call.place.as_ref()?;
    let (kept, _) = places::current(core, &call.project);
    kept.into_iter().find(|place| place.shown.id == shown.id)
}
