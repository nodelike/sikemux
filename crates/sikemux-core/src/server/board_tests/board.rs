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
