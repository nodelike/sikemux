//! The orgs the signed-in account holds a seat in, kept in the account folder. The account says
//! orgs.changed when they change, and the host reads them again then and each time its live
//! connection comes back.

use std::path::Path;

use sikemux_view::device::{AccountOrg, OrgRole};

use super::kept;
use crate::accounts::protocol::{self, OrgMemberships, OrgPlan};
use crate::server::Core;

const FILE: &str = "orgs.json";
const PATH: &str = "/v1/account/orgs";

/// `user_id`'s orgs as clients are sent them; none while the account has not said.
pub async fn read(dir: &Path, user_id: &str) -> Vec<AccountOrg> {
    let Some(kept) = kept::read::<OrgMemberships>(dir, FILE, user_id).await else {
        return Vec::new();
    };
    kept.orgs
        .into_iter()
        .map(|org| AccountOrg {
            id: org.id,
            name: org.name,
            role: match org.role {
                protocol::OrgRole::Owner => OrgRole::Owner,
                protocol::OrgRole::Admin => OrgRole::Admin,
                protocol::OrgRole::User | protocol::OrgRole::Unknown => OrgRole::User,
            },
            active: org.plan == OrgPlan::Active,
        })
        .collect()
}

pub async fn refresh(core: &Core) {
    kept::refresh::<OrgMemberships>(core, PATH, FILE).await;
}
