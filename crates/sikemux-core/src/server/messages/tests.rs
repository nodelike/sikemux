use std::collections::{HashMap, HashSet};

use serde_json::json;
use sikemux_fold::{ChatEventKind, Fold};
use sikemux_process::sync::Locked;
use sikemux_view::chat::LetterKind;
use sikemux_view::turn::{TurnPhase, TurnState};
use sikemux_wire::protocol::PermissionMode;

use super::delivery::waiting_texts;
use super::format::{joined, read_as};
use super::reports::calm_due_in;
use super::*;
use crate::server::parts::Parts;
use crate::server::ServerCore;

/// The person asked Chief for a redone design, which Chief handed @Okkoto. Its calm report
/// waited as one line while @Shun was at work, and Chief only read it when the person asked
/// where @Okkoto had gone.
#[test]
fn a_calm_report_on_the_persons_work_wakes_its_lead_while_another_worker_works() {
    use crate::server::board::asked::{Asked, Holder};
    use crate::server::board_state::BoardParts;
    let core = Core::new(Default::default(), None).unwrap();
    (core.boards().state.locked().made("/shop")).ask(
        "Redo the pages and calls design".to_owned(),
        1,
        Asked {
            to: "chief".to_owned(),
            owners: vec![Holder {
                agent: "okkoto".to_owned(),
                before: None,
            }],
            ..Asked::default()
        },
    );
    let worker = |id: &str| Listed {
        name: Some(id.to_owned()),
        project: "/shop".to_owned(),
        cli: "claude".to_owned(),
        title: None,
        status: sikemux_wire::protocol::AgentStatus::Idle,
        parent: Some("chief".to_owned()),
        active_at: None,
        rank: 0,
        reach: Reach::Chat,
        agent_id: id.to_owned(),
    };
    let agents = [worker("okkoto"), worker("shun"), worker("kiki")];
    let shun_works = TurnState {
        state: TurnPhase::Working,
        since: 1,
        on: None,
        wait: None,
        call: None,
        done: None,
        stopped: None,
        failed: None,
        seen: false,
    };
    let turns = HashMap::from([("shun".to_owned(), shun_works)]);
    assert_eq!(
        calm_due_in(&core, &turns, &agents, "okkoto", "chief", 7),
        CalmWait::At { at: 7 }
    );
    assert_eq!(
        calm_due_in(&core, &turns, &agents, "kiki", "chief", 7),
        CalmWait::UntilSiblingsIdle {
            capped_at: 7 + CALM_CEILING_MS
        }
    );
}

/// Workers handed the person's work lost their wake mark once their earlier ask entered the
/// running build, after an answer turn with no open in-turn ask, on a follow-up send without
/// `work: true`, across a restart/update, or after closing and resuming; upkeep stays held,
/// and calm reports arriving together still batch into one wake.
#[test]
fn persons_work_mark_survives_follow_up_restart_and_resume() {
    use crate::server::names::{Starting, Worker};

    let dir = tempfile::tempdir().unwrap();
    let open = |ids: &[&str]| ids.iter().map(|id| (*id).to_owned()).collect();
    let worker_listed = |id: &str| Listed {
        name: Some(id.to_owned()),
        project: "/shop".to_owned(),
        cli: "claude".to_owned(),
        title: None,
        status: sikemux_wire::protocol::AgentStatus::Idle,
        parent: Some("chief".to_owned()),
        active_at: None,
        rank: 0,
        reach: Reach::Chat,
        agent_id: id.to_owned(),
    };
    let busy = TurnState {
        state: TurnPhase::Working,
        since: 1,
        on: None,
        wait: None,
        call: None,
        done: None,
        stopped: None,
        failed: None,
        seen: false,
    };
    let turns = HashMap::from([("shun".to_owned(), busy)]);
    let agents = [
        worker_listed("koizumi"),
        worker_listed("upkeep"),
        worker_listed("shun"),
    ];

    let core = Core::new(Default::default(), Some(dir.path())).unwrap();
    core.names().give(
        Starting {
            agent_id: "chief",
            project: "/shop".into(),
            provider: Some("claude"),
            resume_id: None,
        },
        open(&["chief"]),
    );
    for (id, session) in [("koizumi", "ses-koizumi"), ("upkeep", "ses-upkeep")] {
        core.names()
            .hire(
                Worker {
                    agent_id: id,
                    lead: "chief",
                    project: None,
                    provider: "claude",
                    session: Some(session),
                    reports: true,
                    task: None,
                },
                open(&["chief"]),
            )
            .unwrap();
    }
    core.names().settle(open(&["chief", "koizumi", "upkeep"]));

    let hand_request = |method: &str, params: Value| HarnessRequest {
        id: "1".to_owned(),
        project: "/shop".to_owned(),
        agent_id: Some("chief".to_owned()),
        method: method.to_owned(),
        params,
    };

    // Marked with work: true even without an in-turn ask on the board (e.g. answer turn).
    super::super::harness::handing(
        &core,
        &hand_request("agent.send", json!({ "to": "koizumi", "work": true })),
        Ok(json!({ "agentId": "koizumi" })),
    )
    .unwrap();
    assert_eq!(
        calm_due_in(&core, &turns, &agents, "koizumi", "chief", 10),
        CalmWait::At { at: 10 }
    );
    assert_eq!(
        calm_due_in(&core, &turns, &agents, "upkeep", "chief", 10),
        CalmWait::UntilSiblingsIdle {
            capped_at: 10 + CALM_CEILING_MS
        }
    );

    // A follow-up agent_send without work keeps the mark.
    super::super::harness::handing(
        &core,
        &hand_request(
            "agent.send",
            json!({ "to": "koizumi", "text": "follow up" }),
        ),
        Ok(json!({ "agentId": "koizumi" })),
    )
    .unwrap();
    assert_eq!(
        calm_due_in(&core, &turns, &agents, "koizumi", "chief", 11),
        CalmWait::At { at: 11 }
    );

    // Closing and restarting the core (update/handover) preserves the mark on disk.
    core.names().settle(open(&["chief", "upkeep"]));
    drop(core);

    let restarted = Core::new(Default::default(), Some(dir.path())).unwrap();
    let returning =
        restarted
            .names()
            .returning_id(Some("claude"), "ses-koizumi", open(&["chief", "upkeep"]));
    assert_eq!(returning, "koizumi");
    restarted
        .names()
        .hire(
            Worker {
                agent_id: &returning,
                lead: "chief",
                project: None,
                provider: "claude",
                session: Some("ses-koizumi"),
                reports: true,
                task: None,
            },
            open(&["chief", "upkeep"]),
        )
        .unwrap();
    assert_eq!(
        calm_due_in(&restarted, &turns, &agents, "koizumi", "chief", 12),
        CalmWait::At { at: 12 }
    );
    assert_eq!(
        calm_due_in(&restarted, &turns, &agents, "upkeep", "chief", 12),
        CalmWait::UntilSiblingsIdle {
            capped_at: 12 + CALM_CEILING_MS
        }
    );

    // Resuming the same session under a fresh id also inherits the mark.
    restarted
        .names()
        .settle(open(&["chief", "koizumi", "upkeep"]));
    restarted.names().settle(open(&["chief", "upkeep"]));
    restarted
        .names()
        .hire(
            Worker {
                agent_id: "koizumi-resumed",
                lead: "chief",
                project: None,
                provider: "claude",
                session: Some("ses-koizumi"),
                reports: true,
                task: None,
            },
            open(&["chief", "upkeep"]),
        )
        .unwrap();
    let resumed_agents = [
        worker_listed("koizumi-resumed"),
        worker_listed("upkeep"),
        worker_listed("shun"),
    ];
    assert_eq!(
        calm_due_in(
            &restarted,
            &turns,
            &resumed_agents,
            "koizumi-resumed",
            "chief",
            13
        ),
        CalmWait::At { at: 13 }
    );

    // Several due calm reports arriving together are taken and joined into one wake.
    let mailbox = &restarted.harness().mailbox;
    for (from, line) in [
        ("koizumi-resumed", "- @Koizumi: Landed docs."),
        ("kirito", "- @Kirito: Landed composer fix."),
    ] {
        mailbox.wait(
            "chief",
            Queued {
                text: line.to_owned(),
                origin: Some(Origin {
                    from: from.to_owned(),
                    kind: MessageKind::Report,
                    awaited: false,
                    calm: Some(CalmWait::At { at: 13 }),
                }),
                mark: None,
                paths: Vec::new(),
            },
        );
    }
    assert_eq!(mailbox.calm_until("chief"), Some(13));
    let batch = mailbox.take("chief");
    assert_eq!(batch.len(), 2);
    let combined = joined(&batch);
    assert!(combined.contains("@Koizumi: Landed docs."));
    assert!(combined.contains("@Kirito: Landed composer fix."));
    assert_eq!(combined.matches("<agent-message").count(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn persons_work_mark_clears_when_task_closes_or_reaches_running_build() {
    use super::super::board::{add_card, change_card, claim_card, Adding, By, CardStatus, Changes};
    use super::super::board_state::BoardParts;
    use super::super::names::{Starting, Worker};
    use super::super::now::asks;
    use super::super::workspace::{add_project, AddProjectParams, AddProjectParamsExt};
    use super::super::{BuildIdentity, ServerCore};
    use sikemux_git::Git;
    use sikemux_view::turn::{TurnPhase, TurnState};

    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("shop");
    std::fs::create_dir_all(&repo).unwrap();
    let git = |args: &[&str]| {
        let mut all = vec!["-c", "user.name=t", "-c", "user.email=t@t"];
        all.extend(args);
        Git::quick(&repo).trimmed(&all).expect("git")
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["commit", "-q", "--allow-empty", "-m", "start"]);
    let shipped = git(&["rev-parse", "HEAD"]);

    let open =
        |ids: &[&str]| -> HashSet<String> { ids.iter().map(|id| (*id).to_owned()).collect() };
    let worker_listed = |id: &str| Listed {
        name: Some(id.to_owned()),
        project: repo.to_string_lossy().into_owned(),
        cli: "claude".to_owned(),
        title: None,
        status: sikemux_wire::protocol::AgentStatus::Idle,
        parent: Some("chief".to_owned()),
        active_at: None,
        rank: 0,
        reach: Reach::Chat,
        agent_id: id.to_owned(),
    };
    let busy = TurnState {
        state: TurnPhase::Working,
        since: 1,
        on: None,
        wait: None,
        call: None,
        done: None,
        stopped: None,
        failed: None,
        seen: false,
    };
    let turns = HashMap::from([("shun".to_owned(), busy)]);
    let agents = [worker_listed("koizumi"), worker_listed("shun")];

    let build = BuildIdentity {
        commit: shipped.clone(),
        ..BuildIdentity::default()
    };
    let core = Core::new(build, Some(dir.path())).unwrap();
    let project = add_project(&core, AddProjectParams::at(repo.clone()))
        .await
        .unwrap();
    let folder = project.path.to_string_lossy().into_owned();
    for id in ["chief", "koizumi"] {
        let draft = json!({
            "id": id,
            "provider": "claude",
            "project": project.id,
            "cwd": project.path,
            "title": id,
            "permissionMode": "auto",
        });
        core.workspaces()
            .put_agent_draft(draft.as_object().cloned().unwrap())
            .unwrap();
    }
    core.names().give(
        Starting {
            agent_id: "chief",
            project: folder.clone(),
            provider: Some("claude"),
            resume_id: None,
        },
        open(&["chief"]),
    );
    core.names()
        .hire(
            Worker {
                agent_id: "koizumi",
                lead: "chief",
                project: None,
                provider: "claude",
                session: Some("ses-koizumi"),
                reports: true,
                task: None,
            },
            open(&["chief"]),
        )
        .unwrap();
    core.names().settle(open(&["chief", "koizumi"]));

    let hand_request = |params: Value| HarnessRequest {
        id: "1".to_owned(),
        project: folder.clone(),
        agent_id: Some("chief".to_owned()),
        method: "agent.send".to_owned(),
        params,
    };

    // 1. Worker closes its own board task during a turn: this turn's finish still wakes,
    // then clears so a later upkeep turn stays held.
    super::super::harness::handing(
        &core,
        &hand_request(json!({ "to": "koizumi", "work": true })),
        Ok(json!({ "agentId": "koizumi" })),
    )
    .unwrap();
    let (card_id, _) = add_card(
        &core,
        &folder,
        &By::Agent("chief".to_owned()),
        Adding {
            title: "Fix docs".to_owned(),
            details: String::new(),
            acceptance: String::new(),
            parent: None,
            depends_on: Vec::new(),
            ticket: None,
            trigger: None,
            fired_on: None,
            about: None,
            ci: None,
        },
        None,
    )
    .unwrap();
    claim_card(&core, &folder, "koizumi", card_id).unwrap();
    change_card(
        &core,
        &folder,
        &By::Agent("koizumi".to_owned()),
        card_id,
        Changes {
            status: Some(CardStatus::Done),
            ..Changes::default()
        },
    )
    .unwrap();
    assert_eq!(
        calm_due_in(&core, &turns, &agents, "koizumi", "chief", 10),
        CalmWait::At { at: 10 }
    );
    asks::turn_reported(&core, "koizumi");
    assert_eq!(
        calm_due_in(&core, &turns, &agents, "koizumi", "chief", 11),
        CalmWait::UntilSiblingsIdle {
            capped_at: 11 + CALM_CEILING_MS
        }
    );

    // 2. Lead closes a worker's task while idle: clears immediately; new work: true wakes again.
    super::super::harness::handing(
        &core,
        &hand_request(json!({ "to": "koizumi", "work": true })),
        Ok(json!({ "agentId": "koizumi" })),
    )
    .unwrap();
    let (card2, _) = add_card(
        &core,
        &folder,
        &By::Agent("chief".to_owned()),
        Adding {
            title: "Second fix".to_owned(),
            details: String::new(),
            acceptance: String::new(),
            parent: None,
            depends_on: Vec::new(),
            ticket: None,
            trigger: None,
            fired_on: None,
            about: None,
            ci: None,
        },
        None,
    )
    .unwrap();
    claim_card(&core, &folder, "koizumi", card2).unwrap();
    change_card(
        &core,
        &folder,
        &By::Agent("chief".to_owned()),
        card2,
        Changes {
            status: Some(CardStatus::Done),
            ..Changes::default()
        },
    )
    .unwrap();
    assert_eq!(
        calm_due_in(&core, &turns, &agents, "koizumi", "chief", 12),
        CalmWait::UntilSiblingsIdle {
            capped_at: 12 + CALM_CEILING_MS
        }
    );

    // 3. An ask whose commit reaches the running build clears the mark, unless another open
    // ask remains; once that ask closes too, later upkeep stays held.
    let ask1 = asks::asked(&core, "chief", "First ask", &[], None).unwrap();
    asks::handed(
        &core,
        "chief",
        &["koizumi".into()],
        &[ask1],
        None,
        true,
        true,
    );
    asks::turn_ended(&core, "chief");
    let ask2 = asks::asked(&core, "chief", "Second ask", &[], None).unwrap();
    asks::handed(
        &core,
        "chief",
        &["koizumi".into()],
        &[ask2],
        None,
        true,
        true,
    );
    asks::turn_ended(&core, "chief");
    {
        let mut state = core.boards().state.locked();
        let board = state.of_mut(&folder).unwrap();
        let card = board.card_mut(ask1).unwrap();
        card.change_asked(13, |asked| {
            asked.commit = Some(shipped.clone());
            asked.landed_by = Some("koizumi".to_owned());
            asked.owners.clear();
        });
    }
    asks::build_moved(&core, &project.path);
    assert_eq!(
        calm_due_in(&core, &turns, &agents, "koizumi", "chief", 14),
        CalmWait::At { at: 14 }
    );
    asks::done(
        &core,
        &HarnessRequest {
            id: "2".to_owned(),
            project: folder.clone(),
            agent_id: Some("chief".to_owned()),
            method: "task.update".to_owned(),
            params: json!({ "asked": ask2 }),
        },
    )
    .unwrap();
    assert_eq!(
        calm_due_in(&core, &turns, &agents, "koizumi", "chief", 15),
        CalmWait::UntilSiblingsIdle {
            capped_at: 15 + CALM_CEILING_MS
        }
    );

    // 4. Single ask reaching the running build clears the mark on its own; new work: true
    // sets it again.
    let ask3 = asks::asked(&core, "chief", "Third ask", &[], None).unwrap();
    asks::handed(
        &core,
        "chief",
        &["koizumi".into()],
        &[ask3],
        None,
        true,
        true,
    );
    asks::turn_ended(&core, "chief");
    assert_eq!(
        calm_due_in(&core, &turns, &agents, "koizumi", "chief", 16),
        CalmWait::At { at: 16 }
    );
    {
        let mut state = core.boards().state.locked();
        let board = state.of_mut(&folder).unwrap();
        let card = board.card_mut(ask3).unwrap();
        card.change_asked(16, |asked| {
            asked.commit = Some(shipped.clone());
            asked.landed_by = Some("koizumi".to_owned());
            asked.owners.clear();
        });
    }
    asks::build_moved(&core, &project.path);
    assert_eq!(
        calm_due_in(&core, &turns, &agents, "koizumi", "chief", 17),
        CalmWait::UntilSiblingsIdle {
            capped_at: 17 + CALM_CEILING_MS
        }
    );
    super::super::harness::handing(
        &core,
        &hand_request(json!({ "to": "koizumi", "work": true })),
        Ok(json!({ "agentId": "koizumi" })),
    )
    .unwrap();
    assert_eq!(
        calm_due_in(&core, &turns, &agents, "koizumi", "chief", 18),
        CalmWait::At { at: 18 }
    );
}

/// A lead's chat drew a worker's report as the person's own message, raw tags and all,
/// while the lead had a call about that worker open.
#[tokio::test]
async fn a_report_from_a_worker_a_call_is_open_about_still_reads_as_its_letter() {
    let core = Core::new(Default::default(), None).unwrap();
    core.harness().questions.hold(json!({
        "id": "question-1",
        "agentId": "lead",
        "name": "Chief",
        "cli": "claude",
        "project": "widget",
        "asked": "Plain failure messages",
        "text": "Plain failure messages",
        "choices": [],
        "blocked": false,
        "assumption": null,
        "secret": false,
        "at": 7,
        "about": { "agentId": "worker", "name": "Masayuki", "cli": "claude" },
        "stage": { "stage": "lead", "leadId": "lead", "leadName": "Chief", "endedAt": null },
    }));
    let mut letter = Letter {
        kind: MessageKind::Message,
        from_id: "worker".to_owned(),
        from_name: "Masayuki".to_owned(),
        cli: "claude".to_owned(),
        text: "PR 3 is up.".to_owned(),
        at: 7,
        note: None,
        answers: None,
        puts: None,
        paths: Vec::new(),
        waits: false,
        awaited: false,
        calm: None,
    };

    let text = read_as(&core, "lead", &mut letter);
    assert!(
        text.contains("question-1"),
        "the lead is told of its open call"
    );

    let mut fold = Fold::new(PermissionMode::WorkspaceWrite);
    fold.apply(0, ChatEventKind::Prompt, &json!({ "text": text }), 1_000);
    fold.flush();
    let rows = fold.view(false).rows;
    let letters = &rows.first().expect("the message").letters;
    assert_eq!(letters.len(), 1);
    assert_eq!(letters[0].kind, LetterKind::Message);
    assert_eq!(letters[0].name, "Masayuki");
    assert_eq!(letters[0].text, "PR 3 is up.");
}

/// A worker's turn ended while its lead rested from its own, so the report waited. The
/// lead wrote to the worker again, that turn ended too, and its report took the place of
/// the first, which the lead never read.
#[tokio::test]
async fn a_report_still_waiting_is_kept_when_the_workers_next_turn_ends() {
    let core = Core::new(Default::default(), None).unwrap();
    let mailbox = &core.harness().mailbox;
    let report = |text: &str| Queued {
        text: text.to_owned(),
        origin: Some(Origin {
            from: "agent-otter".to_owned(),
            kind: MessageKind::Report,
            awaited: false,
            calm: None,
        }),
        mark: None,
        paths: Vec::new(),
    };

    let first = "The payment webhook is built and tested, and a preview deploy is up.";
    let second = "Both pieces are finished. The details are in my message to Chief.";
    mailbox.wait("agent-chief", report(first));
    let (waiting, dropped) = mailbox.wait("agent-chief", report(second));

    assert!(dropped.is_empty(), "the first turn's report was dropped");
    assert_eq!(waiting, 2);
    assert_eq!(waiting_texts(&core, "agent-chief"), [first, second]);
}

/// CI failed after a closed worker's push, and the note Sikemux sent its lead in the
/// worker's place showed as "@agent-8f42…" instead of the worker's name.
#[test]
fn a_letter_sent_for_a_closed_agent_carries_its_name() {
    use crate::server::names::{Starting, Worker};

    let core = Core::new(Default::default(), None).unwrap();
    let open = |ids: &[&str]| ids.iter().map(|id| (*id).to_owned()).collect();
    let lead = Starting {
        agent_id: "agent-lead",
        project: "/p".into(),
        provider: Some("claude"),
        resume_id: None,
    };
    core.names().give(lead, open(&["agent-lead"]));
    let worker = Worker {
        agent_id: "agent-8f42",
        lead: "agent-lead",
        project: None,
        provider: "claude",
        session: Some("ses-1"),
        reports: true,
        task: None,
    };
    let name = core
        .names()
        .hire(worker, open(&["agent-lead"]))
        .unwrap()
        .name;
    core.names().settle(open(&["agent-lead", "agent-8f42"]));
    core.names().settle(open(&["agent-lead"]));

    let letter = letter_from(
        &core,
        &[],
        MessageKind::Report,
        "agent-8f42".to_owned(),
        "CI on 7b4da8e54 failed:".to_owned(),
        None,
    );
    let mut fold = Fold::new(PermissionMode::WorkspaceWrite);
    fold.apply(
        0,
        ChatEventKind::Prompt,
        &json!({ "text": wrapped(&letter) }),
        1_000,
    );
    fold.flush();
    let rows = fold.view(false).rows;
    let letters = &rows.first().expect("the message").letters;
    assert_eq!(letters[0].name, name);
    assert_ne!(letters[0].name, "agent-8f42");
}
