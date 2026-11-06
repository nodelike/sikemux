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
            Asking::Keys => Cheatsheet::new("Git pane keybindings", &KEYS)
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
                    "(optional) stash message",
                    "",
                    move |pane, message, cx| {
                        let message = (!message.is_empty()).then_some(message);
                        pane.run(
                            format!("stashing ({mode})"),
                            git_call::StashPush {
                                repo: pane.repo.clone(),
                                mode: mode.to_owned(),
                                message: message.clone(),
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
            "Stash",
            vec![
                Pick::new(
                    's',
                    "stash everything (working tree + index + untracked)",
                    stash("all", "Stash everything"),
                ),
                Pick::new(
                    'i',
                    "stash staged only",
                    stash("staged", "Stash staged changes"),
                ),
                Pick::new(
                    'u',
                    "stash unstaged only (keep index)",
                    stash("unstaged", "Stash unstaged changes"),
                ),
            ],
            cx,
        );
    }

    /// The ways to throw away the chosen file's changes, each asked about once more.
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
        let files = format!("{count} file{}", if count > 1 { "s" } else { "" });
        let label = if count > 1 {
            format!("Discard {count} files")
        } else {
            format!(
                "Discard changes — {}",
                first.rsplit('/').next().unwrap_or(&first)
            )
        };
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
                            format!("discarding {files} ({mode})"),
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
                Pick::new('d', "discard all changes", apply("all", format!("This will discard BOTH staged and unstaged changes in {files}. Cannot be undone."), "discard"))
                    .hint("staged + worktree")
                    .destructive(),
                Pick::new('u', "discard unstaged changes", apply("unstaged", format!("Discard unstaged changes in {files}? (staged changes preserved)"), "discard"))
                    .destructive(),
                Pick::new('s', "unstage changes", apply("staged", format!("Unstage {files} from the index? (worktree preserved)"), "unstage")),
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
        self.menu("Stashes", picks, cx);
    }

    pub(crate) fn stash_menu_of(&mut self, stash: &GitStash, cx: &mut Context<Self>) {
        let (repo, refname, sha) = (self.repo.clone(), stash.refname.clone(), stash.sha.clone());
        let apply = {
            let call = git_call::StashApply {
                repo: repo.clone(),
                refname: refname.clone(),
                sha: sha.clone(),
            };
            stash_step(format!("applying {refname}"), call)
        };
        let pop = {
            let call = git_call::StashPop {
                repo: repo.clone(),
                refname: refname.clone(),
                sha: sha.clone(),
            };
            stash_step(format!("popping {refname}"), call)
        };
        let drop = {
            let call = git_call::StashDrop {
                repo,
                refname: refname.clone(),
                sha,
            };
            stash_step(format!("dropping {refname}"), call)
        };
        let (branched, renamed) = (stash.clone(), stash.clone());
        let refname = stash.refname.clone();
        self.menu(
            format!("{} · {}", stash.refname, stash.message),
            vec![
                Pick::new('a', "apply stash", apply).hint("keep stash"),
                Pick::new('p', "pop stash", pop).hint("apply + drop"),
                Pick::new('b', "create branch from stash", move |pane, cx| {
                    let stash = branched.clone();
                    pane.prompt(
                        format!("Branch from {}", stash.refname),
                        "branch name",
                        "",
                        move |pane, name, cx| {
                            if name.is_empty() {
                                return;
                            }
                            pane.run(
                                format!("branching from {}", stash.refname),
                                git_call::StashBranch {
                                    repo: pane.repo.clone(),
                                    refname: stash.refname.clone(),
                                    sha: stash.sha.clone(),
                                    name: name.clone(),
                                },
                                cx,
                                |pane, _, cx| pane.read_stashes(cx),
                            );
                        },
                        cx,
                    );
                }),
                Pick::new('r', "rename stash", move |pane, cx| {
                    let stash = renamed.clone();
                    pane.prompt(
                        format!("Rename {}", stash.refname),
                        "",
                        stash.message.clone(),
                        move |pane, message, cx| {
                            if message.is_empty() || message == stash.message {
                                return;
                            }
                            pane.run(
                                format!("renaming {}", stash.refname),
                                git_call::StashRename {
                                    repo: pane.repo.clone(),
                                    refname: stash.refname.clone(),
                                    sha: stash.sha.clone(),
                                    new_message: message.clone(),
                                },
                                cx,
                                |pane, _, cx| pane.read_stashes(cx),
                            );
                        },
                        cx,
                    );
                }),
                Pick::new('d', "drop stash", move |pane, cx| {
                    let drop = drop.clone();
                    pane.ask(
                        format!("Drop {refname}?"),
                        "This permanently deletes the stash entry.",
                        "drop",
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
                    "create branch from commit",
                    GitPane::branch_from_commit,
                ),
                Pick::new('r', "reset to this commit", GitPane::reset_menu)
                    .hint("choose soft / mixed / hard"),
                Pick::new('v', "revert this commit", GitPane::revert_commit)
                    .hint("new inverse commit"),
                Pick::new('y', "copy hash", move |pane, cx| {
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
            format!("Branch from {hash}"),
            "branch name",
            "",
            move |pane, name, cx| {
                if name.is_empty() {
                    return;
                }
                pane.run(
                    format!("creating {name} from {hash}"),
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
                pane.confirm(
                    format!("{mode} reset to {hash}?"),
                    body,
                    format!("{mode} reset"),
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
            format!("Reset to {hash}"),
            vec![
                Pick::new('s', "soft reset", reset("soft", "Moves HEAD to this commit and keeps all later changes staged.")).hint("keep changes staged"),
                Pick::new('m', "mixed reset", reset("mixed", "Moves HEAD to this commit and keeps all later changes in the working tree.")).hint("keep changes unstaged"),
                Pick::new('h', "hard reset", reset("hard", "Moves HEAD to this commit and discards all later changes from the index and working tree."))
                    .hint("discard later changes")
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
            format!("Revert {hash}?"),
            "Creates a new commit that reverses this commit. Existing history is preserved.",
            "revert",
            false,
            move |pane, cx| {
                pane.run(
                    format!("reverting {hash}"),
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
            "Add remote",
            "name (e.g. upstream) — then you'll enter the URL",
            "",
            |pane, name, cx| {
                if name.is_empty() {
                    return;
                }
                pane.prompt(
                    format!("Add remote · {name}"),
                    "URL (https://… or git@…)",
                    "",
                    move |pane, url, cx| {
                        if url.is_empty() {
                            return;
                        }
                        pane.run(
                            format!("adding remote {name}"),
                            git_call::RemoteAdd {
                                repo: pane.repo.clone(),
                                name: name.clone(),
                                url: url.clone(),
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
