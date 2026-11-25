use super::Question;
use sikemux_process::sync::Locked;

#[tokio::test]
async fn a_worker_stopped_for_its_leads_answer_waits_on_the_lead_rather_than_being_done() {
    use crate::server::activity::{Activity, ActivityCore};
    use crate::server::parts::Parts;
    use crate::server::{Core, ServerCore};
    use sikemux_view::turn::{TurnPhase, TurnWaitKind};

    let core = Core::new(Default::default(), None).unwrap();
    core.note_activity("worker", Activity::Started);
    let asked: Question = serde_json::from_value(serde_json::json!({
        "id": "question-1",
        "agentId": "worker",
        "name": "Garou",
        "cli": "opencode",
        "project": "widget",
        "asked": "Buy the display font for the welcome screen, $40?",
        "text": "Buy the display font for the welcome screen, $40?",
        "choices": [],
        "blocked": true,
        "assumption": null,
        "secret": false,
        "at": 7,
        "stage": { "stage": "lead", "leadId": "lead", "leadName": "Mumen", "endedAt": null },
    }))
    .expect("a question");
    core.harness().questions.state.locked().open.push(asked);
    core.note_activity("worker", Activity::Finished { cancelled: false });

    let turn = crate::server::turn_state::now(&core, "worker").expect("the turn ended");
    assert_eq!(turn.state, TurnPhase::Waiting);
    let wait = turn.wait.expect("what it waits on");
    assert_eq!(wait.kind, TurnWaitKind::Agent);
    assert_eq!(wait.what, "Mumen");
    assert_eq!(wait.started_at, 7);
}

/// A lead asked the person about its worker's question, then ended its turn, and the
/// person got the worker's question as a second call.
#[tokio::test(flavor = "multi_thread")]
async fn a_lead_asking_the_person_about_its_workers_question_raises_one_call() {
    use crate::server::parts::Parts;
    use crate::server::workspace::{AddProjectParams, AddProjectParamsExt};
    use crate::server::workspace_state::WorkspaceParts;
    use crate::server::{Core, ServerCore};

    let core = Core::new(Default::default(), None).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let mut all = vec!["-c", "user.name=test", "-c", "user.email=test@localhost"];
        all.extend(args);
        crate::git::Git::at(dir.path()).text(&all).expect("git");
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["commit", "-q", "--allow-empty", "-m", "start"]);
    let project =
        crate::server::workspace::add_project(&core, AddProjectParams::at(dir.path().to_owned()))
            .await
            .expect("the project");
    for agent in ["worker", "lead"] {
        let draft: serde_json::Map<String, serde_json::Value> =
            serde_json::from_value(serde_json::json!({
                "id": agent,
                "provider": "claude",
                "project": project.id,
                "cwd": dir.path(),
                "title": agent,
                "permissionMode": "auto",
            }))
            .expect("a draft");
        core.workspaces().put_agent_draft(draft).expect("the agent");
    }
    let workers: Question = serde_json::from_value(serde_json::json!({
        "id": "question-worker",
        "agentId": "worker",
        "name": "Nia",
        "cli": "claude",
        "project": "widget",
        "asked": "Which call page, A, B or C?",
        "text": "Which call page, A, B or C?",
        "choices": [],
        "blocked": true,
        "at": 7,
        "stage": { "stage": "lead", "leadId": "lead", "leadName": "Chief", "endedAt": 1 },
    }))
    .expect("a question");
    let mut leads = workers.clone();
    leads.id = "question-lead".to_owned();
    leads.agent_id = "lead".to_owned();
    leads.name = "Chief".to_owned();
    leads.stage = super::Stage::Person { lead: None };
    leads.about = Some(super::CallAsker {
        agent_id: "worker".to_owned(),
        name: "Nia".to_owned(),
        cli: "claude".to_owned(),
        chief: false,
    });
    let questions = &core.harness().questions;
    questions
        .state
        .locked()
        .open
        .extend([workers, leads.clone()]);

    super::hold_for(&core, &leads, None);
    super::settle(&core);

    let for_person: Vec<String> = questions.for_person().into_iter().map(|q| q.id).collect();
    assert_eq!(for_person, ["question-lead"]);
}

/// @Chief asked about @Floch's mockup, and the person looked for the ask in @Floch's chat,
/// where it was not; @Floch found no open ask in agent_list and told them there was none. Their
/// message to @Floch cleared the ask's pick, and the note telling @Chief was dropped.
#[tokio::test(flavor = "multi_thread")]
async fn agents_know_of_calls_open_about_their_work() {
    use crate::server::parts::Parts;
    use crate::server::workspace::{AddProjectParams, AddProjectParamsExt};
    use crate::server::workspace_state::WorkspaceParts;
    use crate::server::{Core, ServerCore};

    let core = Core::new(Default::default(), None).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let project =
        crate::server::workspace::add_project(&core, AddProjectParams::at(dir.path().to_owned()))
            .await
            .expect("the project");
    for (agent, name) in [("worker", "Floch"), ("lead", "Chief")] {
        let draft: serde_json::Map<String, serde_json::Value> =
            serde_json::from_value(serde_json::json!({
                "id": agent,
                "provider": "claude",
                "project": project.id,
                "cwd": dir.path(),
                "title": agent,
                "permissionMode": "auto",
                "terminal": true,
            }))
            .expect("a draft");
        core.workspaces().put_agent_draft(draft).expect("the agent");
        let adopted = crate::server::names::Adopted {
            agent_id: agent,
            name,
            project: dir.path().to_string_lossy().into_owned(),
            parent: None,
            provider: "claude",
            reports: false,
            task: None,
        };
        core.names().adopt(adopted, [agent.to_owned()].into());
    }
    let question: Question = serde_json::from_value(serde_json::json!({
        "id": "question-lead",
        "agentId": "lead",
        "name": "Chief",
        "cli": "claude",
        "project": dir.path(),
        "header": "Link preview",
        "asked": "Which mockup?",
        "text": "Which mockup?",
        "choices": [{ "label": "One", "recommended": true }, { "label": "Two" }],
        "blocked": false,
        "at": 7,
        "stage": { "stage": "person" },
        "about": { "agentId": "worker", "name": "Floch", "cli": "claude" },
    }))
    .expect("a question");
    let questions = &core.harness().questions;
    questions.state.locked().open.push(question);
    super::settle(&core);

    let rows = crate::server::roster::rows(&core, None, "worker");
    let lead = rows
        .iter()
        .find(|row| row["agentId"] == "lead")
        .expect("the lead");
    assert_eq!(
        lead["asksPerson"],
        serde_json::json!(["question-lead \"Link preview\""])
    );

    let told = super::open_calls(&core, "worker").expect("a note");
    assert!(
        told.text
            .contains("question-lead \"Link preview\" from @Chief"),
        "{}",
        told.text
    );

    crate::server::messages::person_wrote(&core, "worker");
    super::settle(&core);
    let mailbox = &core.harness().mailbox;
    for _ in 0..100 {
        if mailbox.has_waiting("lead") {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("the asker never hears its pick was cleared");
}

/// @Chief raised two calls about @Lille's two questions, and @Lille got the first call's
/// answer for both.
#[tokio::test(flavor = "multi_thread")]
async fn two_calls_about_one_worker_each_answer_their_own_question() {
    use super::{AnswerParams, AnswersParams};
    use crate::server::parts::Parts;
    use crate::server::workspace::{AddProjectParams, AddProjectParamsExt};
    use crate::server::workspace_state::WorkspaceParts;
    use crate::server::{Core, ServerCore};

    let core = Core::new(Default::default(), None).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let mut all = vec!["-c", "user.name=test", "-c", "user.email=test@localhost"];
        all.extend(args);
        crate::git::Git::at(dir.path()).text(&all).expect("git");
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["commit", "-q", "--allow-empty", "-m", "start"]);
    let project =
        crate::server::workspace::add_project(&core, AddProjectParams::at(dir.path().to_owned()))
            .await
            .expect("the project");
    for agent in ["worker", "lead"] {
        let draft: serde_json::Map<String, serde_json::Value> =
            serde_json::from_value(serde_json::json!({
                "id": agent,
                "provider": "claude",
                "project": project.id,
                "cwd": dir.path(),
                "title": agent,
                "permissionMode": "auto",
            }))
            .expect("a draft");
        core.workspaces().put_agent_draft(draft).expect("the agent");
    }
    let workers = |id: &str, asked: &str| -> Question {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "agentId": "worker",
            "name": "Lille",
            "cli": "claude",
            "project": project.id,
            "asked": asked,
            "text": asked,
            "choices": [],
            "blocked": true,
            "at": 7,
            "stage": { "stage": "lead", "leadId": "lead", "leadName": "Chief", "endedAt": null },
        }))
        .expect("a question")
    };
    let questions = &core.harness().questions;
    questions.state.locked().open.extend([
        workers("question-search", "Where does search go?"),
        workers("question-tasks", "T1 or T2 for teammates' tasks?"),
    ]);
    let mut calls = Vec::new();
    for (id, about, text) in [
        ("1", "question-search", "Where does search go?"),
        ("2", "question-tasks", "T1 or T2 for teammates' tasks?"),
    ] {
        let request = crate::cli::protocol::HarnessRequest {
            id: id.to_owned(),
            project: project.id.clone(),
            agent_id: Some("lead".to_owned()),
            method: "agent.ask".to_owned(),
            params: serde_json::json!({ "question": text, "about": about }),
        };
        let asked = super::ask(&core, &request).await.expect("the call");
        calls.push(asked["questionId"].as_str().unwrap().to_owned());
    }

    for (call, answer) in calls.iter().zip(["In the triggers view", "T1"]) {
        super::answer_all(
            &core,
            AnswersParams {
                answers: vec![AnswerParams {
                    id: call.clone(),
                    answer: Some(answer.to_owned()),
                    ..AnswerParams::default()
                }],
            },
        )
        .await
        .expect("the answer");
    }

    let answered: Vec<(String, Option<String>)> = (questions.state.locked().answered.iter())
        .filter(|answered| answered.question.agent_id == "worker")
        .map(|answered| (answered.question.id.clone(), answered.answer.clone()))
        .collect();
    assert_eq!(
        answered,
        [
            (
                "question-search".to_owned(),
                Some("In the triggers view".to_owned())
            ),
            ("question-tasks".to_owned(), Some("T1".to_owned())),
        ]
    );
}

/// An access call asking the person to press a button in a page only they reach kept its
/// "Merged both" answer out of the chat, as if it were a password. Only a secret ask's is.
#[tokio::test(flavor = "multi_thread")]
async fn only_a_secret_asks_answer_stays_out_of_every_chat() {
    use crate::server::parts::Parts;
    use crate::server::{Core, ServerCore};
    use sikemux_view::calls::CallKind;

    let data = tempfile::tempdir().unwrap();
    let core = Core::new(Default::default(), Some(data.path())).unwrap();
    let ask = |id: &str, params: serde_json::Value| crate::cli::protocol::HarnessRequest {
        id: id.to_owned(),
        project: data.path().to_string_lossy().into_owned(),
        agent_id: Some("chief".to_owned()),
        method: "agent.ask".to_owned(),
        params,
    };
    let questions = &core.harness().questions;

    let press = ask(
        "1",
        serde_json::json!({
            "question": "Press Squash merge on both PRs?",
            "kind": "access",
            "choices": ["Merged both", "Wait"],
        }),
    );
    super::ask(&core, &press).await.expect("the access ask");
    let asked = questions.state.locked().open[0].clone();
    let call = asked.call();
    assert!(
        !asked.secret && !call.secret,
        "an access ask is not a secret"
    );
    assert_eq!(call.kind, CallKind::Access);
    assert_eq!(asked.view().secret_note, None);

    let password = ask(
        "2",
        serde_json::json!({ "question": "The staging password?", "secret": true }),
    );
    super::ask(&core, &password).await.expect("the secret ask");
    let asked = questions.state.locked().open[1].clone();
    let call = asked.call();
    assert!(asked.secret && call.secret);
    assert!(asked.view().secret_note.is_some());

    let typed = "hunter2-staging";
    questions.keep_secret(&asked, typed).expect("kept");
    let sent = questions.hide_secrets(&format!("Use {typed} to sign in."));
    assert_eq!(sent, format!("Use {} to sign in.", super::HIDDEN));
}

fn asked_in(reply: &str) -> Vec<(String, bool)> {
    (super::asked_in(reply).into_iter())
        .map(|ask| (ask.text, ask.polar))
        .collect()
}

#[test]
fn a_worker_left_waiting_on_the_person_says_it_waits() {
    for said in [
        "Card #265 isn't part of my benchmark work, so I'm not taking it. Still waiting for the person to sign in on sanctum.",
        "I'm leaving card #266 for someone else; it's unrelated to the benchmark. Still waiting for the person to sign in on sanctum before I can start the build test.",
        "Fixed the probe.\n\nOnce that's done I'll check the setup and rerun the build test and the typing-lag test.",
    ] {
        assert!(super::waits_in(said), "{said}");
    }
}

/// The shape of a reply whose three offers showed as one question, cut inside its bold.
const THREE_OFFERS: &str = "The claims read is the slow part now.\n\n\
    - Text-ad Projects load in 1.8 s. **Ship it to dev now,** alongside the worker deploy \
    that's still waiting on your approval?\n\
    - Ship the speed fix to dev?\n\
    - **Profile the claims read** to get text-ad Projects under a second?\n";

#[test]
fn each_ask_of_a_reply_is_a_question_of_its_own_in_plain_words() {
    let asked = asked_in(THREE_OFFERS);
    assert_eq!(
        asked,
        vec![
            (
                "Ship it to dev now, alongside the worker deploy that's still waiting on \
                 your approval?"
                    .to_owned(),
                true
            ),
            ("Ship the speed fix to dev?".to_owned(), true),
            (
                "Profile the claims read to get text-ad Projects under a second?".to_owned(),
                true
            ),
        ]
    );
}

#[test]
fn no_sentence_ends_inside_emphasis_or_code() {
    let asked = asked_in("Tests pass. **Merged, then deployed.** Which of `a. b` or `c` next?");
    assert_eq!(asked, vec![("Which of a. b or c next?".to_owned(), false)]);
    let asked = asked_in("Tests pass. **Merged, then deployed. Check it?**");
    assert_eq!(
        asked,
        vec![("Merged, then deployed. Check it?".to_owned(), true)]
    );
}

/// The shape of a reply whose question showed as "Yes or no?", with no Yes or No to tap.
const BARE_LAST_SENTENCE: &str = "**Your call: fix the repeated number now?** It prints \
    twice on any shape with a big number.\n\n\
    The catch is the cost of recomposing. This is the cheapest time to do it. Yes or no?\n";

#[test]
fn a_last_sentence_that_means_nothing_alone_shows_the_question_it_stands_for() {
    assert_eq!(
        asked_in(BARE_LAST_SENTENCE),
        vec![("Your call: fix the repeated number now?".to_owned(), true)]
    );
    assert_eq!(
        asked_in("Should I fix it?\n\nYes or no?"),
        vec![("Should I fix it?".to_owned(), true)]
    );
    assert_eq!(
        asked_in("It's fixed. Yes or no?"),
        vec![("It's fixed. Yes or no?".to_owned(), true)]
    );
}

/// The shape of a reply whose question showed as "Is that okay?", saying nothing of what.
const OKAY_AFTER_A_PROPOSAL: &str = "**Who owns #5 (no indication on Product claim stills)?** \
    I'd have Satoru find the cause first, and Medical or you decide whether it blocks. \
    Is that okay?\n";

#[test]
fn a_question_about_what_was_just_said_shows_with_it() {
    assert_eq!(
        asked_in(OKAY_AFTER_A_PROPOSAL),
        vec![(
            "Who owns #5 (no indication on Product claim stills)? I'd have Satoru find the \
             cause first, and Medical or you decide whether it blocks. Is that okay?"
                .to_owned(),
            false
        )]
    );
    assert_eq!(
        asked_in("We could cache it.\n\nThoughts?"),
        vec![("We could cache it. Thoughts?".to_owned(), false)]
    );
}
