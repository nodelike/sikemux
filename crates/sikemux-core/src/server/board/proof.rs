//! Each slice's proof in the description of its pull request: the block `slice/words.rs`
//! draws, written whenever what it shows changes and rewritten in place on later pushes. A
//! slice is matched to an open pull request by its latest commit, or is the one Auto opened.
//! The first write to each pull request waits for the person to post the exact block.

use crate::server::parts::Parts;
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::Arc;

use sikemux_process::sync::Locked;

use super::slice::{with_proof, KeptPicture, SlicePull};
use super::{AutoPull, CardStatus, State};
use crate::server::codehost::repo as codehost;
use crate::server::Core;

#[derive(Default)]
pub(super) struct Proofs {
    /// What each slice's proof showed when it was last handed over to be written, by project
    /// and card, so it is written once per change.
    said: HashMap<(String, u64), u64>,
    /// Projects whose proofs are being written.
    writing: HashSet<String>,
}

struct Due {
    card: u64,
    head: String,
    pull: Option<AutoPull>,
}

/// Starts writing the proofs that changed since they were last written, one project at a time.
pub(super) fn due(core: &Arc<Core>) {
    let mut writes = Vec::new();
    {
        let mut state = core.boards().state.locked();
        let State { boards, proofs, .. } = &mut *state;
        for (project, board) in boards.iter() {
            if proofs.writing.contains(project) {
                continue;
            }
            let mut dues = Vec::new();
            for card in (board.cards.iter()).filter(|card| card.status == CardStatus::Progress) {
                let Some(slice) = &card.slice else { continue };
                let Some(head) = slice.head() else { continue };
                let pull = (slice.pull.as_ref().map(|pull| pull.at.clone()))
                    .or_else(|| card.auto.as_ref().and_then(|run| run.pull.clone()));
                let mut shows = DefaultHasher::new();
                slice.proof_block(card.id, "", None).hash(&mut shows);
                slice.proof_picture().map(|(id, _)| id).hash(&mut shows);
                (head, pull.as_ref().map(|pull| &pull.url)).hash(&mut shows);
                let shows = shows.finish();
                if proofs.said.insert((project.clone(), card.id), shows) != Some(shows) {
                    dues.push(Due {
                        card: card.id,
                        head: head.to_owned(),
                        pull,
                    });
                }
            }
            if !dues.is_empty() {
                proofs.writing.insert(project.clone());
                writes.push((project.clone(), dues));
            }
        }
    }
    for (project, dues) in writes {
        let core = core.clone();
        tokio::spawn(async move {
            write(&core, &project, dues).await;
            core.boards().state.locked().proofs.writing.remove(&project);
        });
    }
}

/// A pull request was just opened, so slices with none look for theirs again.
pub(crate) fn pull_opened(core: &Arc<Core>) {
    {
        let mut state = core.boards().state.locked();
        let State { boards, proofs, .. } = &mut *state;
        proofs.said.retain(|(project, card), _| {
            (boards.get(project).and_then(|board| board.card(*card).ok()))
                .and_then(|card| card.slice.as_ref())
                .is_some_and(|slice| slice.pull.is_some())
        });
    }
    core.boards().wake.now();
}

async fn write(core: &Arc<Core>, project: &str, dues: Vec<Due>) {
    let mut found = None;
    for due in dues {
        let pull = match due.pull {
            Some(pull) => pull,
            None => {
                if found.is_none() {
                    found = Some(
                        codehost::of_project(core, project)
                            .await
                            .map_err(String::from),
                    );
                }
                let Some(Ok(repo)) = &found else { return };
                match codehost::pull_at(repo, &due.head).await {
                    Ok(Some(pull)) => pull,
                    Ok(None) => continue,
                    Err(error) => return failed(project, due.card, &error),
                }
            }
        };
        if let Err(error) = rewrite(core, project, due.card, pull).await {
            failed(project, due.card, &error);
        }
    }
}

fn failed(project: &str, card: u64, error: &str) {
    eprintln!(
        "sikemux core: the pull request of card #{card} in {project} kept its old proof: {error}"
    );
}

/// Where the person stands on writing to a slice's pull request.
#[derive(Clone, Copy, PartialEq)]
enum Consent {
    /// Not asked yet: the person is asked before anything is written.
    Unasked,
    Asked,
    Posted,
    Skipped,
}

async fn rewrite(core: &Arc<Core>, project: &str, card: u64, pull: AutoPull) -> Result<(), String> {
    let (consent, picture, kept, owner) = {
        let state = core.boards().state.locked();
        let board = state.boards.get(project).ok_or("the board is gone")?;
        let held = board.card(card)?;
        let slice = held.slice.as_ref().ok_or("the slice is gone")?;
        let linked = (slice.pull.as_ref()).filter(|linked| linked.at.url == pull.url);
        let consent = match linked {
            Some(linked) if linked.skipped => Consent::Skipped,
            Some(linked) if linked.posted => Consent::Posted,
            Some(linked) if linked.asked => Consent::Asked,
            _ => Consent::Unasked,
        };
        let kept = linked.and_then(|linked| linked.picture.clone());
        let picture = (slice.proof_picture()).map(|(id, path)| (id, path.to_owned()));
        let owner = (held.owner_id().map(str::to_owned))
            .or_else(|| held.auto.as_ref().map(|run| run.agent.clone()));
        (consent, picture, kept, owner)
    };
    match consent {
        Consent::Skipped | Consent::Asked => return Ok(()),
        Consent::Unasked => return ask(core, project, card, pull, picture, owner),
        Consent::Posted => {}
    }
    let kept = match picture {
        Some((id, _)) if kept.as_ref().is_some_and(|kept| kept.evidence == id) => kept,
        Some((id, path)) => {
            let name = picture_name(&pull, card, id, &path);
            match codehost::attach(core, &pull, &path, &name).await {
                Ok(url) => Some(KeptPicture { evidence: id, url }),
                Err(error) => {
                    failed(project, card, &format!("its picture was not kept: {error}"));
                    None
                }
            }
        }
        None => None,
    };
    let Some(body) = codehost::open_body(core, &pull).await? else {
        return Ok(());
    };
    let block = {
        let state = core.boards().state.locked();
        let slice = (state.boards.get(project))
            .and_then(|board| board.card(card).ok())
            .and_then(|card| card.slice.as_ref())
            .ok_or("the slice is gone")?;
        let repo = format!("{}/{}", pull.owner, pull.name);
        slice.proof_block(card, &repo, kept.as_ref().map(|kept| kept.url.as_str()))
    };
    let written = with_proof(&body, &block);
    if written != body {
        codehost::describe(core, &pull, &written).await?;
    }
    core.boards().edit_if(project, card, |card| {
        let linked = card.slice.as_mut()?.pull.as_mut()?;
        if linked.picture == kept {
            return None;
        }
        linked.picture = kept;
        Some(())
    });
    Ok(())
}

/// Puts the exact block to the person before the first write to the pull request, on behalf
/// of the agent that owns the slice, and keeps the pull request as asked about.
fn ask(
    core: &Arc<Core>,
    project: &str,
    card: u64,
    pull: AutoPull,
    picture: Option<(u64, String)>,
    owner: Option<String>,
) -> Result<(), String> {
    let Some(owner) = owner else {
        return Ok(());
    };
    let block = {
        let state = core.boards().state.locked();
        let slice = (state.boards.get(project))
            .and_then(|board| board.card(card).ok())
            .and_then(|card| card.slice.as_ref())
            .ok_or("the slice is gone")?;
        let repo = format!("{}/{}", pull.owner, pull.name);
        slice.proof_block(card, &repo, picture.as_ref().map(|(_, path)| path.as_str()))
    };
    let mut text = format!(
        "Sikemux would add this to the description of pull request #{} ({}), below what it says, and keep it up to date on later pushes:\n\n{block}",
        pull.number, pull.url
    );
    if picture.is_some() {
        text.push_str("\n\nPosting also keeps its screenshot on the code host: on GitHub, on a `sikemux-evidence` branch of the repository.");
    }
    let header = format!("Post proof to #{}", pull.number);
    let asked = ProofAsk {
        project: project.to_owned(),
        card,
    };
    (core.board_above().questions_put_proof_to_person)(core, &owner, asked, header, text)?;
    core.boards().edit(project, card, |card| {
        if let Some(slice) = &mut card.slice {
            slice.pull = Some(SlicePull {
                at: pull,
                picture: None,
                asked: true,
                posted: false,
                skipped: false,
            });
        }
    });
    Ok(())
}

/// The person posted the slice's proof to its pull request, or skipped it for good.
pub(crate) fn answered(core: &Arc<Core>, asked: &ProofAsk, post: bool) {
    core.boards().edit_if(&asked.project, asked.card, |card| {
        let linked = card.slice.as_mut()?.pull.as_mut()?;
        linked.asked = false;
        linked.posted = post;
        linked.skipped = !post;
        Some(())
    });
    (core.boards().state.locked().proofs.said).remove(&(asked.project.clone(), asked.card));
    core.boards().wake.now();
}

/// A name for the picture no other slice's or pull request's shares.
fn picture_name(pull: &AutoPull, card: u64, evidence: u64, path: &str) -> String {
    let extension = Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("png")
        .to_ascii_lowercase();
    format!(
        "pull-{}-card-{card}-evidence-{evidence}.{extension}",
        pull.number
    )
}

/// A slice of the board at `project`, by its card.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProofAsk {
    pub project: String,
    pub card: u64,
}
