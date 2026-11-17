//! What the signed-in account's plan allows, kept on disk so a host that cannot reach the
//! account still knows. The account says plan.changed when it changes, and the host reads it
//! again then and each time its live connection comes back.

use std::path::Path;

use super::kept;
use crate::accounts::protocol::{Allowance, PlanFeature, PlanId};
use crate::server::Core;

const FILE: &str = "allowance.json";
const PATH: &str = "/v1/account/allowance";

/// What `user_id`'s plan allows, or nothing while the account has not said.
pub async fn read(dir: &Path, user_id: &str) -> Option<Allowance> {
    kept::read(dir, FILE, user_id).await
}

/// Whether the account may use Pro's features without paying for them: before billing is live,
/// or while Pro is a gift.
pub fn pro_given(allowance: &Allowance) -> bool {
    let has_pro = allowance.features.iter().any(|feature| {
        matches!(
            feature,
            PlanFeature::Sync
                | PlanFeature::MoveWork
                | PlanFeature::HistorySearch
                | PlanFeature::OwnTriggers
        )
    });
    allowance.gift_ends.is_some() || (allowance.plan == PlanId::Free && has_pro)
}

/// Asks the account what its plan allows and shows the answer. Until it has said, everything is
/// allowed.
pub async fn refresh(core: &Core) {
    kept::refresh::<Allowance>(core, PATH, FILE).await;
}
