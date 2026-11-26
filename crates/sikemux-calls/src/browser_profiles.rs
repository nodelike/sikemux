use serde::{Deserialize, Serialize};
use sikemux_view::browser_profiles::{BrowserProfile, ProfileAccess, ResolvedBrowserProfile};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserProfilesView {
    pub profiles: Vec<BrowserProfile>,
    pub global_default_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserProfileUpsertParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<ProfileAccess>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_space_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserProfileUpsertAnswer {
    pub profile: BrowserProfile,
    pub profiles: Vec<BrowserProfile>,
    pub global_default_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserProfileDeleteParams {
    pub id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserProfileDeleteAnswer {
    pub deleted_id: String,
    pub profiles: Vec<BrowserProfile>,
    pub global_default_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserProfileSetGlobalParams {
    pub id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserProfileSetSpaceParams {
    pub space_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    #[serde(default)]
    pub create_matching: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserProfileSetSpaceAnswer {
    pub space_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<BrowserProfile>,
    pub created: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserProfileSetProjectParams {
    pub project: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserProfileResolveParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explicit_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab_override: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab_override_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserProfileResolveAnswer {
    pub resolved: ResolvedBrowserProfile,
    pub inherited: ResolvedBrowserProfile,
    pub profiles: Vec<BrowserProfile>,
    pub global_default_id: String,
}
