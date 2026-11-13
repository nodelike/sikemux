//! What the pane does to the repository, one operation at a time, and the row moves it queues.

use super::*;
use serde::de::DeserializeOwned;
use sikemux_calls::edits::CommitParams;
use sikemux_calls::git::PullRequestUrlParams;

/// What an operation's answer says to the person; most say nothing.
pub(crate) trait Said {
    fn said(&self) -> &str {
        ""
    }
}

impl Said for () {}

impl Said for String {
    fn said(&self) -> &str {
        self
    }
}

impl GitPane {
    /// Runs the git operation `call`, as [`run_call`] does, for a `call` that is its own params.
    pub(super) fn run<C>(
        &mut self,
        label: impl Into<SharedString>,
        call: C,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, Result<C::Answer, String>, &mut Context<Self>) + 'static,
    ) where
        C: Call<Params = C> + Serialize + 'static,
        C::Answer: DeserializeOwned + Said + 'static,
    {
        self.run_call::<C>(label, call, cx, then);
    }

    /// Runs one git operation at a time, saying in the head what it is while it runs and
    /// what went wrong if it fails, and what it answered if it says anything.
    pub(crate) fn run_call<C>(
        &mut self,
        label: impl Into<SharedString>,
        params: C::Params,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, Result<C::Answer, String>, &mut Context<Self>) + 'static,
    ) where
        C: Call + 'static,
        C::Params: Serialize,
        C::Answer: DeserializeOwned + Said + 'static,
    {
        self.quietly::<C>(label, params, cx, |pane, answer, cx| {
            if let Ok(answer) = &answer {
                pane.done(answer.said(), cx);
            }
            then(pane, answer, cx);
        });
    }

    /// As [`run_call`], leaving what the operation answered for `then` to show.
    pub(crate) fn quietly<C>(
        &mut self,
        label: impl Into<SharedString>,
        params: C::Params,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, Result<C::Answer, String>, &mut Context<Self>) + 'static,
    ) where
        C: Call + 'static,
        C::Params: Serialize,
        C::Answer: DeserializeOwned + Said + 'static,
    {
        if self.busy.is_some() {
            return;
        }
        if let Some(host) = &self.out_of_reach {
            let said = t!(
                "{host} is out of reach, so this waits until it is back.",
                host = host
            );
            self.fail(&said, cx);
            return;
        }
        let label = label.into();
        self.busy = Some(label.clone());
        let logged = self.logged(label);
        let answer = self.core.request_typed::<C>(params);
        cx.spawn(async move |pane, cx| {
            let answer = answer.await;
            pane.update(cx, |pane, cx| {
                pane.busy = None;
                let ended = match &answer {
                    Ok(answer) => Ok(answer.said()),
                    Err(error) => Err(error.as_str()),
                };
                pane.settle(logged, ended);
                if let Err(error) = &answer {
                    pane.fail(error, cx);
                }
                then(pane, answer, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Notes an operation in the command log as running, answering its place there.
    pub(crate) fn logged(&mut self, label: SharedString) -> u64 {
        let at = self.commands.back().map_or(0, |last| last.id + 1);
        self.commands.push_back(Logged {
            id: at,
            time: chrono::Local::now().format("%H:%M:%S").to_string().into(),
            label,
            done: None,
            detail: None,
        });
        while self.commands.len() > LOG_KEPT {
            self.commands.pop_front();
        }
        at
    }

    /// Notes how a logged operation ended, with the first line of what it said.
    pub(crate) fn settle(&mut self, id: u64, ended: Result<&str, &str>) {
        let Some(entry) = self.commands.iter_mut().find(|entry| entry.id == id) else {
            return;
        };
        entry.done = Some(ended.is_ok());
        let first = first_line(ended.unwrap_or_else(|error| error));
        entry.detail = (!first.is_empty()).then(|| first.into());
    }

    /// Says what went wrong, by the first line of it, until it is dismissed.
    pub(crate) fn fail(&self, error: &str, cx: &mut Context<Self>) {
        self.say(toast::error(first_line(error)).autohide(false), cx);
    }

    /// Says what an operation answered, by its first line, when it said anything.
    pub(crate) fn done(&self, said: &str, cx: &mut Context<Self>) {
        if !said.trim().is_empty() {
            self.say(toast::success(first_line(said)), cx);
        }
    }

    pub(crate) fn say(&self, note: Notification, cx: &mut Context<Self>) {
        crate::toast::show(&self.bench, note, cx);
    }

    pub(crate) fn copy(&self, text: String, what: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.say(toast::success(t!("Copied {what}", what = what)), cx);
    }

    /// Moves files between the groups at once; git's next overview confirms the move
    /// or puts them back. Clicks in quick succession wait their turn.
    pub(crate) fn shift<C>(
        &mut self,
        paths: Vec<String>,
        side: Side,
        call: C,
        cx: &mut Context<Self>,
    ) where
        C: Call<Params = C, Answer = ()> + Serialize + 'static,
    {
        if paths.is_empty() {
            return;
        }
        for path in &paths {
            self.moving.insert(path.clone(), side);
        }
        self.queue.push_back(Move {
            paths,
            side,
            label: C::METHOD,
            start: Box::new(move |core| Box::pin(core.request_typed::<C>(call))),
        });
        self.list(cx);
        self.next_move(cx);
    }

    pub(crate) fn next_move(&mut self, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        let Some(next) = self.queue.pop_front() else {
            return;
        };
        self.running = true;
        let logged = self.logged(format!("{} {}", next.label, next.paths.join(" ")).into());
        let answer = (next.start)(&self.core);
        cx.spawn(async move |pane, cx| {
            let answer = answer.await;
            pane.update(cx, |pane, cx| {
                pane.running = false;
                pane.settle(logged, answer.as_ref().map(|_| "").map_err(String::as_str));
                if let Err(error) = &answer {
                    pane.fail(error, cx);
                }
                let later: HashSet<&String> =
                    pane.queue.iter().flat_map(|queued| &queued.paths).collect();
                let settled: Vec<String> = next
                    .paths
                    .into_iter()
                    .filter(|path| {
                        !later.contains(path) && pane.moving.get(path) == Some(&next.side)
                    })
                    .collect();
                pane.moved.extend(settled);
                pane.next_move(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn stage(&mut self, file: &GitFile, cx: &mut Context<Self>) {
        self.shift(
            vec![file.path.clone()],
            Side::Staged,
            git_call::Stage {
                repo: self.repo.clone(),
                path: file.path.clone(),
            },
            cx,
        );
    }

    pub(crate) fn unstage(&mut self, file: &GitFile, cx: &mut Context<Self>) {
        self.shift(
            vec![file.path.clone()],
            Side::Unstaged,
            git_call::Unstage {
                repo: self.repo.clone(),
                path: file.path.clone(),
            },
            cx,
        );
    }

    pub(crate) fn stage_all(&mut self, cx: &mut Context<Self>) {
        let paths = self.paths(GitFile::has_unstaged);
        self.shift(
            paths,
            Side::Staged,
            git_call::StageAll {
                repo: self.repo.clone(),
            },
            cx,
        );
    }

    pub(crate) fn unstage_all(&mut self, cx: &mut Context<Self>) {
        let paths = self.paths(GitFile::is_staged);
        self.shift(
            paths,
            Side::Unstaged,
            git_call::UnstageAll {
                repo: self.repo.clone(),
            },
            cx,
        );
    }

    pub(crate) fn paths(&self, wanted: fn(&GitFile) -> bool) -> Vec<String> {
        self.files()
            .into_iter()
            .filter(wanted)
            .map(|file| file.path)
            .collect()
    }

    pub(crate) fn toggle_stage(&mut self, cx: &mut Context<Self>) {
        if self.range.is_some() {
            let mut paths: Vec<String> = Vec::new();
            for entry in self.in_range() {
                if let Entry::File {
                    side: Side::Unstaged,
                    file,
                } = entry
                {
                    if !paths.contains(&file.path) {
                        paths.push(file.path.clone());
                    }
                }
            }
            self.range = None;
            let call = git_call::StagePaths {
                repo: self.repo.clone(),
                paths: paths.clone(),
            };
            self.shift(paths, Side::Staged, call, cx);
            return;
        }
        match self.chosen().map(|(side, file)| (side, file.clone())) {
            Some((Side::Staged, file)) => self.unstage(&file, cx),
            Some((Side::Unstaged, file)) => self.stage(&file, cx),
            None => {}
        }
    }

    pub(crate) fn discard(&mut self, file: &GitFile, cx: &mut Context<Self>) {
        let name = file
            .path
            .rsplit('/')
            .next()
            .unwrap_or(&file.path)
            .to_owned();
        let body = if file.worktree == GitChange::Untracked {
            t!("The file is new and will be deleted. This can't be undone.")
        } else {
            t!("Unstaged changes to this file will be lost. This can't be undone.")
        };
        let path = file.path.clone();
        self.ask(
            t!("Discard changes to {name}?", name = name),
            body,
            t!("discard"),
            move |pane, cx| {
                pane.run(
                    t!("discarding {path}", path = path),
                    git_call::DiscardFiles {
                        repo: pane.repo.clone(),
                        paths: vec![path.clone()],
                        mode: "unstaged".to_owned(),
                    },
                    cx,
                    |_, _, _| {},
                );
            },
            cx,
        );
    }

    pub(crate) fn discard_all(&mut self, cx: &mut Context<Self>) {
        let unstaged: Vec<GitFile> = self
            .files()
            .into_iter()
            .filter(GitFile::has_unstaged)
            .collect();
        if unstaged.is_empty() {
            return;
        }
        let added = unstaged
            .iter()
            .filter(|file| file.worktree == GitChange::Untracked)
            .count();
        let files = unstaged.len();
        let (title, doing) = match files {
            1 => (
                t!("Discard unstaged changes in 1 file?").to_owned(),
                t!("discarding 1 file").to_owned(),
            ),
            files => (
                t!("Discard unstaged changes in {count} files?", count = files),
                t!("discarding {count} files", count = files),
            ),
        };
        let fate = match added {
            0 => t!("Every file goes back to what is staged or committed.").to_owned(),
            1 => t!("1 new file is deleted and the rest go back to what is staged or committed.").to_owned(),
            added => t!("{added} new files are deleted and the rest go back to what is staged or committed.", added = added),
        };
        let paths: Vec<String> = unstaged.into_iter().map(|file| file.path).collect();
        self.ask(
            title,
            format!(
                "{fate} {}",
                t!("Staged changes are kept. This can't be undone.")
            ),
            t!("discard all"),
            move |pane, cx| {
                pane.run(
                    doing.clone(),
                    git_call::DiscardFiles {
                        repo: pane.repo.clone(),
                        paths: paths.clone(),
                        mode: "unstaged".to_owned(),
                    },
                    cx,
                    |_, _, _| {},
                );
            },
            cx,
        );
    }

    pub(crate) fn commit(&mut self, cx: &mut Context<Self>) {
        let message = self.draft.read(cx).value().trim().to_owned();
        if message.is_empty() || self.busy.is_some() {
            return;
        }
        if self.staged_count() == 0 {
            self.fail(t!("Stage the changes you want to commit first."), cx);
            return;
        }
        let params = CommitParams {
            dir: self.repo.clone().into(),
            message: message.clone(),
        };
        self.run_call::<call::CommitStaged>(
            t!("Commit staged changes"),
            params,
            cx,
            move |pane, answer, cx| {
                if answer.is_ok() && pane.draft.read(cx).value().trim() == message {
                    pane.clear_draft(cx);
                }
            },
        );
    }

    pub(crate) fn generate(&mut self, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        let before = self.draft.read(cx).value().to_string();
        self.generating = true;
        let (provider, label, model) = match self.writer() {
            Some((kind, label, model)) => (Some(kind), label, Some(model)),
            None => (None, "Hermes".to_owned(), None),
        };
        self.quietly::<git_call::AiMessage>(t!("Generate message with {label}", label = label), git_call::AiMessage { repo: self.repo.clone(), provider, model }, cx, move |pane, answer, cx| {
            pane.generating = false;
            let Ok(message) = answer else {
                return;
            };
            if message.trim().is_empty() {
                pane.fail(t!("The provider returned an empty message. Your draft was kept."), cx);
            } else if pane.draft.read(cx).value() != before {
                pane.fail(t!("Your draft changed during generation. It was kept; generate again when ready."), cx);
            } else {
                pane.write(message, cx);
                pane.done(t!("Commit message generated. Review it before committing."), cx);
            }
        });
    }

    pub(crate) fn fetch(&mut self, cx: &mut Context<Self>) {
        self.run(
            t!("fetching all remotes…"),
            git_call::Fetch {
                repo: self.repo.clone(),
                remote: None,
            },
            cx,
            |_, _, _| {},
        );
    }

    pub(crate) fn pull(&mut self, cx: &mut Context<Self>) {
        self.run(
            t!("pulling…"),
            git_call::Pull {
                repo: self.repo.clone(),
            },
            cx,
            |_, _, _| {},
        );
    }

    pub(crate) fn push(&mut self, cx: &mut Context<Self>) {
        self.run(
            t!("pushing…"),
            git_call::Push {
                repo: self.repo.clone(),
            },
            cx,
            |_, _, _| {},
        );
    }

    pub(crate) fn pull_request(&mut self, cx: &mut Context<Self>) {
        let params = PullRequestUrlParams {
            repo: self.repo.clone(),
        };
        self.quietly::<call::GitPullRequestUrl>(
            t!("opening PR…"),
            params,
            cx,
            |_, answer, cx| {
                if let Ok(url) = answer {
                    cx.open_url(&url);
                }
            },
        );
    }
}

/// What a stash's menu row does: runs `call`, then reads the stashes again.
pub(crate) fn stash_step<C>(
    label: String,
    call: C,
) -> impl Fn(&mut GitPane, &mut Context<GitPane>) + Clone + 'static
where
    C: Call<Params = C> + Serialize + Clone + 'static,
    C::Answer: DeserializeOwned + Said + 'static,
{
    move |pane, cx| {
        pane.run(label.clone(), call.clone(), cx, |pane, _, cx| {
            pane.read_stashes(cx)
        });
    }
}

pub(crate) fn first_line(text: &str) -> String {
    text.trim().lines().next().unwrap_or_default().to_owned()
}
