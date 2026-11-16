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

/// On 2026-11-16 the pr-fan trigger's card #170 gathered GitHub's events, a `labeled` one among
/// them, with nobody ever taking it up, so a pull request labelled `triage` was never looked at.
#[tokio::test]
async fn a_triggers_card_is_taken_up_until_someone_holds_it() {
    let core = Core::new(Default::default(), None).unwrap();
    let fire = |action: &str| {
        add_for_trigger(
            &core,
            PROJECT,
            37,
            "pr-fan",
            &format!("{{\"action\":\"{action}\"}}"),
        )
        .unwrap()
    };

    let (id, task) = fire("synchronize").expect("a new card nobody holds is taken up");
    assert!(task.starts_with("pr-fan\n\n"));
    let (again, _) = fire("labeled").expect("an arrival on a card nobody holds is taken up");
    assert_eq!(again, id);

    core.names().pin(HashSet::from(["worker".to_owned()]));
    claim_card(&core, PROJECT, "worker", id).unwrap();
    assert!(fire("labeled").is_none());
}
