//! The questions and menus the pane asks through.

use super::*;

impl GitPane {
    /// Asks whether to go ahead with something that cannot be undone.
    pub(crate) fn ask(
        &mut self,
        title: impl Into<SharedString>,
        body: impl Into<SharedString>,
        yes: impl Into<SharedString>,
        then: impl Fn(&mut GitPane, &mut Context<GitPane>) + 'static,
        cx: &mut Context<Self>,
    ) {
        self.confirm(title, body, yes, true, then, cx);
    }

    pub(crate) fn confirm(
        &mut self,
        title: impl Into<SharedString>,
        body: impl Into<SharedString>,
        yes: impl Into<SharedString>,
        destructive: bool,
        then: impl Fn(&mut GitPane, &mut Context<GitPane>) + 'static,
        cx: &mut Context<Self>,
    ) {
        self.asking = Some(Asking::Confirm {
            title: title.into(),
            body: body.into(),
            yes: yes.into(),
            destructive,
            then: Rc::new(then),
        });
        cx.notify();
    }

    /// Asks for a line of text, such as a name, and goes on with what was typed.
    pub(crate) fn prompt(
        &mut self,
        title: impl Into<SharedString>,
        placeholder: impl Into<SharedString>,
        initial: impl Into<String>,
        then: impl Fn(&mut GitPane, String, &mut Context<GitPane>) + 'static,
        cx: &mut Context<Self>,
    ) {
        self.asking = Some(Asking::Prompt {
            title: title.into(),
            placeholder: placeholder.into(),
            initial: initial.into(),
            field: None,
            then: Rc::new(then),
        });
        cx.notify();
    }

    pub(super) fn menu(
        &mut self,
        title: impl Into<SharedString>,
        choices: Vec<Pick>,
        cx: &mut Context<Self>,
    ) {
        self.asking = Some(Asking::Menu {
            title: title.into(),
            choices,
        });
        cx.notify();
    }

    /// The question being asked, as a dialog, making a prompt's field the first time.
    pub(crate) fn asked(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let pane = cx.entity();
        let done = move |pane: &Entity<GitPane>| {
            let pane = pane.clone();
            move |window: &mut Window, cx: &mut App| {
                pane.update(cx, |pane, cx| {
                    pane.asking = None;
                    window.focus(&pane.focus, cx);
                    cx.notify();
                })
            }
        };
        Some(match self.asking.as_mut()? {
            Asking::Confirm {
                title,
                body,
                yes,
                destructive,
                then,
            } => {
                let (then, going) = (then.clone(), pane.clone());
                Confirm::new(title.clone(), body.clone(), yes.clone())
                    .destructive(*destructive)
                    .on_yes(move |_, cx| {
                        let then = then.clone();
                        going.update(cx, |pane, cx| {
                            pane.asking = None;
                            then(pane, cx);
                            cx.notify();
                        })
                    })
                    .on_no(done(&pane))
                    .into_any_element()
            }
            Asking::Prompt {
                title,
                placeholder,
                initial,
                field,
                then,
            } => {
                let field = field
                    .get_or_insert_with(|| {
                        let (placeholder, initial) = (placeholder.clone(), initial.clone());
                        cx.new(|cx| {
                            let mut state = InputState::new(window, cx).placeholder(placeholder);
                            state.set_value(initial, window, cx);
                            state.focus(window, cx);
                            state
                        })
                    })
                    .clone();
                let (then, going) = (then.clone(), pane.clone());
                Prompt::new(title.clone(), &field)
                    .on_yes(move |typed, _, cx| {
                        let then = then.clone();
                        going.update(cx, |pane, cx| {
                            pane.asking = None;
                            then(pane, typed.trim().to_owned(), cx);
                            cx.notify();
                        })
                    })
                    .on_no(done(&pane))
                    .into_any_element()
            }
            Asking::Keys => Cheatsheet::new(t!("Git pane keybindings"), &KEYS)
                .on_close(done(&pane))
                .into_any_element(),
            Asking::Menu { title, choices } => {
                let choices = choices
                    .iter()
                    .map(|pick| {
                        let (then, going) = (pick.then.clone(), pane.clone());
                        let mut choice = Choice::new(pick.key, pick.label.clone(), move |_, cx| {
                            let then = then.clone();
                            going.update(cx, |pane, cx| {
                                pane.asking = None;
                                then(pane, cx);
                                cx.notify();
                            })
                        })
                        .disabled(pick.disabled);
                        if let Some(hint) = &pick.hint {
                            choice = choice.hint(hint.clone());
                        }
                        if pick.destructive {
                            choice = choice.destructive();
                        }
                        choice
                    })
                    .collect();
                Choices::new(title.clone(), choices)
                    .on_no(done(&pane))
                    .into_any_element()
            }
        })
    }

    pub(crate) fn stash_menu(&mut self, cx: &mut Context<Self>) {
        let stash = |mode: &'static str, title: &'static str| {
            move |pane: &mut GitPane, cx: &mut Context<GitPane>| {
                pane.prompt(
                    title,
                    t!("(optional) stash message"),
                    "",
                    move |pane, message, cx| {
                        let message = (!message.is_empty()).then_some(message);
                        pane.run(
                            t!("stashing ({mode})", mode = mode),
                            git_call::StashPush {
                                repo: pane.repo.clone(),
                                mode: mode.to_owned(),
                                message,
                            },
                            cx,
                            |_, _, _| {},
                        );
                    },
                    cx,
                )
            }
        };
        self.menu(
            t!("Stash"),
            vec![
                Pick::new(
                    's',
                    t!("stash everything (working tree + index + untracked)"),
                    stash("all", t!("Stash everything")),
                ),
                Pick::new(
                    'i',
                    t!("stash staged only"),
                    stash("staged", t!("Stash staged changes")),
                ),
                Pick::new(
                    'u',
                    t!("stash unstaged only (keep index)"),
                    stash("unstaged", t!("Stash unstaged changes")),
                ),
            ],
            cx,
        );
    }

    /// The ways to throw away the changes of the chosen file, or of every file in the
    /// range, each asked about once more.
    pub(crate) fn discard_menu(&mut self, cx: &mut Context<Self>) {
        let mut paths: Vec<String> = Vec::new();
        for entry in self.in_range() {
            if let Entry::File { file, .. } = entry {
                if !paths.contains(&file.path) {
                    paths.push(file.path.clone());
                }
            }
        }
        let Some(first) = paths.first().cloned() else {
            return;
        };
        let count = paths.len();
        let name = first.rsplit('/').next().unwrap_or(&first);
        let one = count == 1;
        let files = if one {
            t!("1 file").to_owned()
        } else {
            t!("{count} files", count = count)
        };
        let label = if one {
            t!("Discard changes — {name}", name = name)
        } else {
            t!("Discard {count} files", count = count)
        };
        let bodies = if one {
            [
                t!("This will discard BOTH staged and unstaged changes in 1 file. Cannot be undone.").to_owned(),
                t!("Discard unstaged changes in 1 file? (staged changes preserved)").to_owned(),
                t!("Unstage 1 file from the index? (worktree preserved)").to_owned(),
            ]
        } else {
            [
                t!("This will discard BOTH staged and unstaged changes in {count} files. Cannot be undone.", count = count),
                t!("Discard unstaged changes in {count} files? (staged changes preserved)", count = count),
                t!("Unstage {count} files from the index? (worktree preserved)", count = count),
            ]
        };
        let [all_body, unstaged_body, staged_body] = bodies;
        let apply = |mode: &'static str, body: String, yes: &'static str| {
            let (paths, label, files) = (paths.clone(), label.clone(), files.clone());
            move |pane: &mut GitPane, cx: &mut Context<GitPane>| {
                let (paths, files) = (paths.clone(), files.clone());
                pane.range = None;
                pane.ask(
                    label.clone(),
                    body.clone(),
                    yes,
                    move |pane, cx| {
                        pane.run(
                            t!("discarding {files} ({mode})", files = files, mode = mode),
                            git_call::DiscardFiles {
                                repo: pane.repo.clone(),
                                paths: paths.clone(),
                                mode: mode.to_owned(),
                            },
                            cx,
                            |_, _, _| {},
                        );
                    },
                    cx,
                )
            }
        };
        self.menu(
            label.clone(),
            vec![
                Pick::new(
                    'd',
                    t!("discard all changes"),
                    apply("all", all_body, t!("discard")),
                )
                .hint(t!("staged + worktree"))
                .destructive(),
                Pick::new(
                    'u',
                    t!("discard unstaged changes"),
                    apply("unstaged", unstaged_body, t!("discard")),
                )
                .destructive(),
                Pick::new(
                    's',
                    t!("unstage changes"),
                    apply("staged", staged_body, t!("unstage")),
                ),
            ],
            cx,
        );
    }

    /// The stashes, each opening what can be done with it.
    pub(crate) fn stashes_menu(&mut self, cx: &mut Context<Self>) {
        let picks = self
            .stashes
            .iter()
            .enumerate()
            .map(|(index, stash)| {
                let key = (index < 9)
                    .then(|| char::from_digit(index as u32 + 1, 10))
                    .flatten();
                let chosen = stash.clone();
                Pick::new(key, stash.message.clone(), move |pane, cx| {
                    pane.stash_menu_of(&chosen, cx)
                })
                .hint(stash.refname.clone())
            })
            .collect();
        self.menu(t!("Stashes"), picks, cx);
    }

    pub(crate) fn stash_menu_of(&mut self, stash: &GitStash, cx: &mut Context<Self>) {
        let (repo, refname, sha) = (self.repo.clone(), stash.refname.clone(), stash.sha.clone());
        let apply = {
            let call = git_call::StashApply {
                repo: repo.clone(),
                refname: refname.clone(),
                sha: sha.clone(),
            };
            stash_step(t!("applying {refname}", refname = refname), call)
        };
        let pop = {
            let call = git_call::StashPop {
                repo: repo.clone(),
                refname: refname.clone(),
                sha: sha.clone(),
            };
            stash_step(t!("popping {refname}", refname = refname), call)
        };
        let drop = {
            let call = git_call::StashDrop {
                repo,
                refname: refname.clone(),
                sha,
            };
            stash_step(t!("dropping {refname}", refname = refname), call)
        };
        let (branched, renamed) = (stash.clone(), stash.clone());
        let refname = stash.refname.clone();
        self.menu(
            format!("{} · {}", stash.refname, stash.message),
            vec![
                Pick::new('a', t!("apply stash"), apply).hint(t!("keep stash")),
                Pick::new('p', t!("pop stash"), pop).hint(t!("apply + drop")),
                Pick::new('b', t!("create branch from stash"), move |pane, cx| {
                    let stash = branched.clone();
                    pane.prompt(
                        t!("Branch from {refname}", refname = stash.refname),
                        t!("branch name"),
                        "",
                        move |pane, name, cx| {
                            if name.is_empty() {
                                return;
                            }
                            pane.run(
                                t!("branching from {refname}", refname = stash.refname),
                                git_call::StashBranch {
                                    repo: pane.repo.clone(),
                                    refname: stash.refname.clone(),
                                    sha: stash.sha.clone(),
                                    name,
                                },
                                cx,
                                |pane, _, cx| pane.read_stashes(cx),
                            );
                        },
                        cx,
                    );
                }),
                Pick::new('r', t!("rename stash"), move |pane, cx| {
                    let stash = renamed.clone();
                    pane.prompt(
                        t!("Rename {refname}", refname = stash.refname),
                        "",
                        stash.message.clone(),
                        move |pane, message, cx| {
                            if message.is_empty() || message == stash.message {
                                return;
                            }
                            pane.run(
                                t!("renaming {refname}", refname = stash.refname),
                                git_call::StashRename {
                                    repo: pane.repo.clone(),
                                    refname: stash.refname.clone(),
                                    sha: stash.sha.clone(),
                                    new_message: message,
                                },
                                cx,
                                |pane, _, cx| pane.read_stashes(cx),
                            );
                        },
                        cx,
                    );
                }),
                Pick::new('d', t!("drop stash"), move |pane, cx| {
                    let drop = drop.clone();
                    pane.ask(
                        t!("Drop {refname}?", refname = refname),
                        t!("This permanently deletes the stash entry."),
                        t!("drop"),
                        move |pane, cx| drop(pane, cx),
                        cx,
                    );
                })
                .destructive(),
            ],
            cx,
        );
    }

    /// What can be done with the commit chosen in the history.
    pub(crate) fn commit_menu(&mut self, cx: &mut Context<Self>) {
        let Some(commit) = self.chosen_commit() else {
            return;
        };
        let full = if commit.full_hash.is_empty() {
            commit.hash.clone()
        } else {
            commit.full_hash.clone()
        };
        let short = commit.hash.clone();
        self.menu(
            format!("{} · {}", commit.hash, commit.subject),
            vec![
                Pick::new(
                    'b',
                    t!("create branch from commit"),
                    GitPane::branch_from_commit,
                ),
                Pick::new('r', t!("reset to this commit"), GitPane::reset_menu)
                    .hint(t!("choose soft / mixed / hard")),
                Pick::new('v', t!("revert this commit"), GitPane::revert_commit)
                    .hint(t!("new inverse commit")),
                Pick::new('y', t!("copy hash"), move |pane, cx| {
                    pane.copy(full.clone(), short.clone(), cx)
                }),
            ],
            cx,
        );
    }

    pub(crate) fn branch_from_commit(&mut self, cx: &mut Context<Self>) {
        if let Some(commit) = self.chosen_commit() {
            self.branch_from(commit.hash, cx);
        }
    }

    pub(super) fn branch_from(&mut self, hash: String, cx: &mut Context<Self>) {
        self.prompt(
            t!("Branch from {hash}", hash = hash),
            t!("branch name"),
            "",
            move |pane, name, cx| {
                if name.is_empty() {
                    return;
                }
                pane.run(
                    t!("creating {name} from {hash}", name = name, hash = hash),
                    git_call::BranchCreate {
                        repo: pane.repo.clone(),
                        name: name.clone(),
                        start_point: Some(hash.clone()),
                    },
                    cx,
                    |_, _, _| {},
                );
            },
            cx,
        );
    }

    pub(crate) fn reset_menu(&mut self, cx: &mut Context<Self>) {
        if let Some(commit) = self.chosen_commit() {
            self.reset_to(commit.hash, cx);
        }
    }

    pub(super) fn reset_to(&mut self, hash: String, cx: &mut Context<Self>) {
        let reset = |mode: &'static str, body: &'static str| {
            let hash = hash.clone();
            move |pane: &mut GitPane, cx: &mut Context<GitPane>| {
                let hash = hash.clone();
                let (title, yes) = match mode {
                    "soft" => (t!("Soft reset to {hash}?", hash = hash), t!("soft reset")),
                    "mixed" => (t!("Mixed reset to {hash}?", hash = hash), t!("mixed reset")),
                    _ => (t!("Hard reset to {hash}?", hash = hash), t!("hard reset")),
                };
                pane.confirm(
                    title,
                    body,
                    yes,
                    mode == "hard",
                    move |pane, cx| {
                        pane.run(
                            format!("reset --{mode} {hash}"),
                            git_call::Reset {
                                repo: pane.repo.clone(),
                                rev: hash.clone(),
                                mode: mode.to_owned(),
                            },
                            cx,
                            |_, _, _| {},
                        );
                    },
                    cx,
                )
            }
        };
        self.menu(
            t!("Reset to {hash}", hash = hash),
            vec![
                Pick::new('s', t!("soft reset"), reset("soft", t!("Moves HEAD to this commit and keeps all later changes staged."))).hint(t!("keep changes staged")),
                Pick::new('m', t!("mixed reset"), reset("mixed", t!("Moves HEAD to this commit and keeps all later changes in the working tree."))).hint(t!("keep changes unstaged")),
                Pick::new('h', t!("hard reset"), reset("hard", t!("Moves HEAD to this commit and discards all later changes from the index and working tree.")))
                    .hint(t!("discard later changes"))
                    .destructive(),
            ],
            cx,
        );
    }

    pub(crate) fn revert_commit(&mut self, cx: &mut Context<Self>) {
        if let Some(commit) = self.chosen_commit() {
            self.revert(commit.hash, cx);
        }
    }

    pub(super) fn revert(&mut self, hash: String, cx: &mut Context<Self>) {
        self.confirm(
            t!("Revert {hash}?", hash = hash),
            t!("Creates a new commit that reverses this commit. Existing history is preserved."),
            t!("revert"),
            false,
            move |pane, cx| {
                pane.run(
                    t!("reverting {hash}", hash = hash),
                    git_call::Revert {
                        repo: pane.repo.clone(),
                        rev: hash.clone(),
                    },
                    cx,
                    |_, _, _| {},
                );
            },
            cx,
        );
    }

    pub(crate) fn add_remote(&mut self, cx: &mut Context<Self>) {
        self.prompt(
            t!("Add remote"),
            t!("name (e.g. upstream) — then you'll enter the URL"),
            "",
            |pane, name, cx| {
                if name.is_empty() {
                    return;
                }
                pane.prompt(
                    t!("Add remote · {name}", name = name),
                    t!("URL (https://… or git@…)"),
                    "",
                    move |pane, url, cx| {
                        if url.is_empty() {
                            return;
                        }
                        pane.run(
                            t!("adding remote {name}", name = name),
                            git_call::RemoteAdd {
                                repo: pane.repo.clone(),
                                name: name.clone(),
                                url,
                            },
                            cx,
                            |pane, _, cx| pane.read_remotes(cx),
                        );
                    },
                    cx,
                );
            },
            cx,
        );
    }
}
