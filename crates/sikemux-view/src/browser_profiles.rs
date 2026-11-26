//! Account-level browser profiles: isolated native browser data stores with
//! agent-readable descriptions and deterministic Tab/Agent -> Playbook -> Space -> Global resolution.

use serde::{Deserialize, Serialize};

pub const MAX_BROWSER_PROFILES: usize = 64;
pub const MAX_PROFILE_NAME_CHARS: usize = 40;
pub const MAX_PROFILE_SLUG_CHARS: usize = 32;
pub const MAX_PROFILE_DESCRIPTION_CHARS: usize = 500;
/// Stable UUID identifying the migrated OS default data store.
pub const DEFAULT_PROFILE_ID: &str = "00000000-0000-4000-8000-000000000001";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProfileAccess {
    #[default]
    Standard,
    AskOutsideProject,
    ManualOnly,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserProfile {
    pub id: String,
    pub slug: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub access: ProfileAccess,
    #[serde(default)]
    pub is_default_store: bool,
    #[serde(default)]
    pub created_at: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProfileSource {
    TabOverride,
    AgentOverride,
    Playbook,
    Space,
    Global,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedBrowserProfile {
    pub profile: BrowserProfile,
    pub source: ProfileSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_label: Option<String>,
}

pub fn default_profile() -> BrowserProfile {
    BrowserProfile {
        id: DEFAULT_PROFILE_ID.to_owned(),
        slug: "default".to_owned(),
        name: "Default".to_owned(),
        description: String::new(),
        access: ProfileAccess::Standard,
        is_default_store: true,
        created_at: 0,
    }
}

pub fn ensure_profiles(profiles: &[BrowserProfile]) -> Vec<BrowserProfile> {
    if profiles.is_empty() {
        return vec![default_profile()];
    }
    let mut list = profiles.to_vec();
    if !list.iter().any(|profile| profile.is_default_store) {
        if let Some(first) = list.first_mut() {
            first.is_default_store = true;
        }
    }
    list
}

pub fn global_default<'a>(
    profiles: &'a [BrowserProfile],
    default_id: Option<&str>,
) -> Option<&'a BrowserProfile> {
    default_id
        .and_then(|id| profiles.iter().find(|profile| profile.id == id))
        .or_else(|| profiles.iter().find(|profile| profile.is_default_store))
        .or_else(|| profiles.first())
}

pub fn slugify(raw: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in raw.trim().chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            out.push(lower);
            prev_dash = false;
        } else if !out.is_empty() && !prev_dash {
            out.push('-');
            prev_dash = true;
        }
        if out.len() >= MAX_PROFILE_SLUG_CHARS {
            break;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "profile".to_owned()
    } else {
        out
    }
}

pub fn unique_slug(desired: &str, existing: &[BrowserProfile], exclude_id: Option<&str>) -> String {
    let base = slugify(desired);
    let taken = |candidate: &str| {
        existing.iter().any(|profile| {
            exclude_id.is_none_or(|id| profile.id != id)
                && profile.slug.eq_ignore_ascii_case(candidate)
        })
    };
    if !taken(&base) {
        return base;
    }
    for n in 2..=999 {
        let suffix = format!("-{n}");
        let max_prefix = MAX_PROFILE_SLUG_CHARS.saturating_sub(suffix.len());
        let prefix: String = base.chars().take(max_prefix).collect();
        let candidate = format!("{}{suffix}", prefix.trim_end_matches('-'));
        if !taken(&candidate) {
            return candidate;
        }
    }
    format!("profile-{}", existing.len() + 1)
}
