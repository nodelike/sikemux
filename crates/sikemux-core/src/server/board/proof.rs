//! Each slice's proof in the description of its pull request: the block `slice/words.rs`
//! draws, written whenever what it shows changes and rewritten in place on later pushes. A
//! slice is matched to an open pull request by its latest commit, or is the one Auto opened.

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
        let mut state = core.boards.state.locked();
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
            core.boards.state.locked().proofs.writing.remove(&project);
        });
    }
}

/// A pull request was just opened, so slices with none look for theirs again.
pub(crate) fn pull_opened(core: &Arc<Core>) {
    {
        let mut state = core.boards.state.locked();
        let State { boards, proofs, .. } = &mut *state;
        proofs.said.retain(|(project, card), _| {
            (boards.get(project).and_then(|board| board.card(*card).ok()))
                .and_then(|card| card.slice.as_ref())
                .is_some_and(|slice| slice.pull.is_some())
        });
    }
    core.boards.wake.now();
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

async fn rewrite(core: &Arc<Core>, project: &str, card: u64, pull: AutoPull) -> Result<(), String> {
    let (picture, kept) = {
        let state = core.boards.state.locked();
        let slice = (state.boards.get(project))
            .and_then(|board| board.card(card).ok())
            .and_then(|card| card.slice.as_ref())
            .ok_or("the slice is gone")?;
        let kept = (slice.pull.as_ref()).and_then(|linked| linked.picture.clone());
        let picture = (slice.proof_picture()).map(|(id, path)| (id, path.to_owned()));
        (picture, kept)
    };
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
        let state = core.boards.state.locked();
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
    core.boards.edit_if(project, card, |card| {
        let slice = card.slice.as_mut()?;
        let same = slice
            .pull
            .as_ref()
            .is_some_and(|linked| linked.at.url == pull.url && linked.picture == kept);
        if same {
            return None;
        }
        slice.pull = Some(SlicePull {
            at: pull,
            picture: kept,
        });
        Some(())
    });
    Ok(())
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
