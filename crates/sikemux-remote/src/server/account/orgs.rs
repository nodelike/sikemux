//! The orgs the signed-in account holds a seat in, kept in the account folder. The account says
//! orgs.changed when they change, and the host reads them again then and each time its live
//! connection comes back.

use std::path::Path;

use sikemux_view::device::{AccountOrg, OrgRole};

use super::{folder, kept};
use crate::accounts::protocol::{self, OrgMemberships, OrgPlan, OrgRuleAdding};
use crate::server::remote_state::RemoteParts;
use crate::server::Core;

const FILE: &str = "orgs.json";
const PATH: &str = "/v1/account/orgs";

/// `user_id`'s orgs as clients are sent them; None while the account has not said.
pub async fn read(dir: &Path, user_id: &str) -> Option<Vec<AccountOrg>> {
    let kept = kept::read::<OrgMemberships>(dir, FILE, user_id).await?;
    let orgs = (kept.orgs.into_iter())
        .map(|org| AccountOrg {
            id: org.id,
            name: org.name,
            role: match org.role {
                protocol::OrgRole::Owner => OrgRole::Owner,
                protocol::OrgRole::Admin => OrgRole::Admin,
                protocol::OrgRole::User | protocol::OrgRole::Unknown => OrgRole::User,
            },
            active: org.plan == OrgPlan::Active && org.hidden != Some(true),
            preview: org.preview == Some(true),
            hidden: org.hidden == Some(true),
        })
        .collect();
    Some(orgs)
}

/// How an org's agents work together, as its owners and admins set it on app.sikemux.com.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OrgPolicy {
    /// A member's rule for a project the org shares waits for an owner or an admin.
    pub admins_approve_rules: bool,
    /// Teammates' agents may ask this person's agents about a task.
    pub asking: bool,
    /// How many questions one person's agents may ask a day, across the org's shared projects.
    pub daily_questions: u32,
}

impl Default for OrgPolicy {
    fn default() -> Self {
        Self {
            admins_approve_rules: false,
            asking: true,
            daily_questions: 30,
        }
    }
}

/// `org_id`'s policy, or the default while this host has not heard it from the account.
pub async fn policy(core: &Core, org_id: &str) -> OrgPolicy {
    let (Some(owner), Some(dir)) = (core.remote().owner(), folder(core)) else {
        return OrgPolicy::default();
    };
    let Some(kept) = kept::read::<OrgMemberships>(&dir, FILE, &owner).await else {
        return OrgPolicy::default();
    };
    kept.orgs
        .into_iter()
        .find(|org| org.id == org_id)
        .map_or_else(OrgPolicy::default, |org| OrgPolicy {
            admins_approve_rules: org.policy.rule_adding != OrgRuleAdding::EveryMember,
            asking: org.policy.asking,
            daily_questions: u32::try_from(org.policy.daily_questions).unwrap_or(0),
        })
}

/// Reads them again, then the vaults of the orgs they name.
pub async fn refresh(core: &Core) {
    kept::refresh::<OrgMemberships>(core, PATH, FILE).await;
    core.vault().orgs().wake();
}
