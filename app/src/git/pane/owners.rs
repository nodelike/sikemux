//! Who wrote which lines of the changes and commits the pane shows.

use super::*;

impl GitPane {
    pub(crate) fn read_owners(&mut self, cx: &mut Context<Self>) {
        let read = |staged: bool| {
            let params = CheckoutParams {
                dir: self.repo.clone().into(),
                staged,
            };
            self.core.request_typed::<call::ChangeOwners>(params)
        };
        let owners = read(false);
        let halves = self
            .files()
            .iter()
            .any(|file| file.is_staged() && file.has_unstaged());
        let staged = halves.then(|| read(true));
        cx.spawn(async move |pane, cx| {
            let owners = owners.await.unwrap_or_default();
            let staged = match staged {
                Some(staged) => staged.await.unwrap_or_default(),
                None => Owners::default(),
            };
            pane.update(cx, |pane, cx| {
                pane.owners = owners;
                pane.staged_owners = staged;
                pane.annotate();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Asks the core which agents the footers of the log's commits name, when those
    /// commits changed.
    pub(crate) fn read_commit_owners(&mut self, cx: &mut Context<Self>) {
        let footered: Vec<String> = self
            .log()
            .iter()
            .filter(|commit| commit.footer)
            .map(|commit| commit.full_hash.clone())
            .collect();
        if footered == self.owners_asked {
            return;
        }
        self.owners_asked = footered.clone();
        if footered.is_empty() {
            self.commit_owners.clear();
            return;
        }
        let answer = self
            .core
            .request_typed::<call::CommitOwners>(CommitsParams {
                dir: self.repo.clone().into(),
                commits: footered.clone(),
            });
        cx.spawn(async move |pane, cx| {
            let owners: HashMap<String, Vec<AgentOwner>> =
                answer.await.unwrap_or_default().into_iter().collect();
            pane.update(cx, |pane, cx| {
                if pane.owners_asked == footered {
                    pane.commit_owners = owners;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Who wrote the lines a diff of `side` shows, as the core last said.
    pub(in crate::git) fn owners_of(&self, side: Option<Side>) -> WhoWrote<'_> {
        if self.review.commit().is_some() {
            WhoWrote::of_commit(&self.commit_lines)
        } else if side == Some(Side::Staged) {
            WhoWrote::of_checkout(&self.staged_owners)
        } else {
            WhoWrote::of_checkout(&self.owners)
        }
    }

    pub(crate) fn read_commit_lines(&mut self, hash: &str, cx: &mut Context<Self>) {
        self.commit_lines = CommitLines::default();
        let answer = self.core.request_typed::<call::CommitLines>(CommitParams {
            dir: self.repo.clone().into(),
            commit: hash.to_owned(),
        });
        let hash = hash.to_owned();
        cx.spawn(async move |pane, cx| {
            let lines = answer.await.unwrap_or_default();
            pane.update(cx, |pane, cx| {
                if pane.asked_commit.as_deref() == Some(hash.as_str()) {
                    pane.commit_lines = lines;
                    pane.annotate();
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Notes beside the lines the review shows who wrote them.
    pub(crate) fn annotate(&mut self) {
        let (owners, staged) = match self.review.commit() {
            Some(_) => (WhoWrote::of_commit(&self.commit_lines), WhoWrote::default()),
            None => (
                WhoWrote::of_checkout(&self.owners),
                WhoWrote::of_checkout(&self.staged_owners),
            ),
        };
        self.review.annotate(&owners, &staged);
    }

    /// The agents that wrote some of `path`'s uncommitted lines, by agent id, with the
    /// agent they ran on, in the order they first appear; and every writer's name.
    pub(crate) fn writers(&self, path: &str) -> (Vec<(String, String)>, Vec<String>) {
        let Some(file) = self.owners.files.iter().find(|file| file.path == path) else {
            return (Vec::new(), Vec::new());
        };
        let mut seen = Vec::new();
        let wrote = file.lines.iter().map(|run| run.author);
        for author in wrote.chain(file.removed.iter().map(|removal| removal.author)) {
            if !seen.contains(&author) {
                seen.push(author);
            }
        }
        let mut faces: Vec<(String, String)> = Vec::new();
        let mut names = Vec::new();
        for author in seen.iter().filter_map(|at| self.owners.authors.get(*at)) {
            names.push(author.name.clone());
            for id in author.agents() {
                if !faces.iter().any(|(known, _)| known == id) {
                    faces.push((id.to_owned(), author.cli.clone()));
                }
            }
        }
        (faces, names)
    }
}
