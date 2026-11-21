use std::collections::HashSet;

use crate::server::board::*;
use crate::server::workspace_state::WorkspaceParts;
use crate::server::Core;
use crate::server::ServerCore;

const PROJECT: &str = "/project";

fn done() -> Changes {
    Changes {
        status: Some(CardStatus::Done),
        ..Changes::default()
    }
}

/// On 2026-11-01 a card stayed in progress after its owner closed: neither the owner's lead,
/// its reopened session nor the card's creator could mark it done.
#[test]
fn a_closed_owners_card_is_free_and_a_live_owners_is_not() {
    let core = Core::new(Default::default(), None).unwrap();
    let creator = By::Agent("creator".to_owned());
    let adding = Adding {
        title: "Stuck".to_owned(),
        ..Adding::default()
    };
    let (id, _) = add_card(&core, PROJECT, &creator, adding, None).unwrap();
    core.names().pin(HashSet::from(["owner".to_owned()]));
    claim_card(&core, PROJECT, "owner", id).unwrap();

    let refused = change_card(&core, PROJECT, &creator, id, done()).map(|_| ());
    assert!(refused.unwrap_err().contains("is owner's"));
    let refused = claim_card(&core, PROJECT, "creator", id);
    assert!(refused.unwrap_err().contains("has #"));

    core.names().pin(HashSet::new());

    let (view, _) = change_card(&core, PROJECT, &creator, id, done()).unwrap();
    assert_eq!(view["status"], "done");
}

fn fire(core: &std::sync::Arc<Core>, action: &str, about: Option<&str>) -> TriggerCard {
    let subject = about.map(|about| Subject {
        about: about.to_owned(),
        head: None,
    });
    let details = format!("{{\"action\":\"{action}\"}}");
    add_for_trigger(
        core,
        PROJECT,
        (37, "this host"),
        "pr-fan",
        &details,
        subject,
    )
    .unwrap()
}

/// On 2026-11-16 the pr-fan trigger's card #170 gathered GitHub's events, a `labeled` one among
/// them, with nobody ever taking it up, so a pull request labelled `triage` was never looked at.
#[tokio::test]
async fn a_triggers_card_is_taken_up_until_someone_holds_it() {
    let core = Core::new(Default::default(), None).unwrap();

    let TriggerCard::TakeUp { card: id, task } = fire(&core, "synchronize", None) else {
        panic!("a new card nobody holds is taken up");
    };
    assert!(task.starts_with("pr-fan\n\n"));
    assert_eq!(fire(&core, "labeled", None), TriggerCard::Kept);

    taken_up(&core, PROJECT, id);
    let TriggerCard::TakeUp { card: again, .. } = fire(&core, "labeled", None) else {
        panic!("an arrival on a card nobody took up is taken up");
    };
    assert_eq!(again, id);

    core.names().pin(HashSet::from(["worker".to_owned()]));
    claim_card(&core, PROJECT, "worker", id).unwrap();
    taken_up(&core, PROJECT, id);
    assert_eq!(fire(&core, "labeled", None), TriggerCard::Kept);
}

/// On 2026-11-21 PR #214 started five agents in a row, one per event, as each marked the
/// trigger's card done and the next event found no open card.
#[tokio::test]
async fn a_burst_of_one_pull_requests_events_starts_one_agent() {
    let core = Core::new(Default::default(), None).unwrap();
    let pr = Some("nodelike/sikemux#214");

    let TriggerCard::TakeUp { card, .. } = fire(&core, "opened", pr) else {
        panic!("the first event on a pull request starts its agent");
    };
    for action in ["labeled", "review_requested", "synchronize", "labeled"] {
        assert_eq!(fire(&core, action, pr), TriggerCard::Kept);
    }

    core.names().pin(HashSet::from(["Hoji".to_owned()]));
    claim_card(&core, PROJECT, "Hoji", card).unwrap();
    taken_up(&core, PROJECT, card);
    let by = By::Agent("Hoji".to_owned());
    change_card(&core, PROJECT, &by, card, done()).unwrap();

    for action in ["edited", "synchronize"] {
        let TriggerCard::Owner {
            card: held, agent, ..
        } = fire(&core, action, pr)
        else {
            panic!("a later event goes to the pull request's agent");
        };
        assert_eq!((held, agent.as_str()), (card, "Hoji"));
    }

    let TriggerCard::TakeUp { card: other, .. } =
        fire(&core, "opened", Some("nodelike/sikemux#215"))
    else {
        panic!("another pull request gets an agent of its own");
    };
    assert_ne!(other, card);
}
