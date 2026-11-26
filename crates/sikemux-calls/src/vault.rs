use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct RecoverParams {
    pub key: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ShownStatus {
    /// This host is on no account.
    #[default]
    Off,
    /// The account's vault waits for a host that holds its key to let this one in.
    Waiting,
    Open,
    Failed,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShownVault {
    pub status: ShownStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed: Option<String>,
    /// How many of the account's hosts hold the vault's key.
    pub holders: usize,
    /// The recovery key of the vault this host made, until the person says they saved it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<String>,
    /// The recovery key waits to be saved, but only this computer's own window shows it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub recovery_in_window: bool,
    /// The vault's sign-ins, which every host on the account uses.
    pub sign_ins: Vec<ShownSignIn>,
    /// The sign-ins that stay on this host, out of the vault.
    pub kept: Vec<ShownSignIn>,
    /// New sign-ins on this host stay here.
    pub keep_new: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShownSignIn {
    pub plugin: String,
    pub identity: String,
    pub label: String,
}

/// Keeps a sign-in on this host and out of the vault, or puts it back in.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeepParams {
    pub plugin: String,
    pub identity: String,
    pub kept: bool,
}

#[derive(Serialize, Deserialize)]
pub struct KeepNewParams {
    pub on: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OrgVaultStatus {
    /// No owner or admin has written to it yet.
    #[default]
    Unmade,
    /// This host has not been let in yet.
    Waiting,
    Open,
}

/// An org's vault, as this host holds it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShownOrgVault {
    pub org: String,
    pub name: String,
    pub status: OrgVaultStatus,
    /// How many of the members' hosts hold its key.
    pub holders: usize,
    /// The org's Team plan is paid. While it is not, agents are handed none of its entries.
    pub active: bool,
    /// The account is an owner or admin there, so this host may add and remove entries.
    pub writes: bool,
    pub model_keys: Vec<ShownOrgModelKey>,
    /// The entries that do not open here, for an owner or admin to remove, by id.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unreadable: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed: Option<String>,
    /// Why this host is past the most computers of one account the vault opens on, and what frees
    /// a place for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full: Option<String>,
    /// While it waits, to an owner or admin: the computers that hold the key and would let this
    /// one in, its own account's and the org's owners' and admins'.
    pub openers: Vec<ShownOrgOpener>,
    /// This host may make the vault again now: it waits, may write, and no opener is online.
    pub remakes: bool,
    /// When an owner's or admin's computer last made the vault again, in ms since the epoch.
    pub remade_at: Option<u64>,
    /// While it waits: what the person pastes on a computer that holds the key to let this one in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

/// A computer that holds an org's key and would let this host in.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShownOrgOpener {
    pub host: String,
    /// The member it is on, by name; None when it is this account's or the name is not known.
    pub whose: Option<String>,
    pub yours: bool,
    /// It is on the org's owner's account, else an admin's or this account's.
    pub owner: bool,
    pub online: bool,
    /// When it was last online, in ms since the epoch; None when it never was.
    pub last_online: Option<u64>,
}

/// A model provider's key the org's admins added for every member. Never the key.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShownOrgModelKey {
    pub id: String,
    /// The catalogue provider it is a key of.
    pub provider: String,
    pub label: String,
    /// The provider id agents are started on to use it.
    pub route: String,
}

#[derive(Serialize, Deserialize)]
pub struct OrgModelKeyAdd {
    pub org: String,
    pub provider: String,
    pub key: String,
}

#[derive(Serialize, Deserialize)]
pub struct OrgParams {
    pub org: String,
}

/// Lets in the computer that shows `code`.
#[derive(Serialize, Deserialize)]
pub struct OrgLetIn {
    pub org: String,
    pub code: String,
}

/// The computer let in, as far as this host knows it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OrgLetInAnswer {
    pub computer: Option<String>,
    /// The member it is on, by name; None when it is this account's or the name is not known.
    pub member: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct OrgEntryParams {
    pub org: String,
    pub id: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VaultCategory {
    #[default]
    Details,
    Secrets,
    Payments,
    Documents,
}

impl VaultCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Details => "details",
            Self::Secrets => "secrets",
            Self::Payments => "payments",
            Self::Documents => "documents",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum VaultScope {
    #[default]
    Personal,
    #[serde(rename_all = "camelCase")]
    Org {
        org_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        org_name: Option<String>,
    },
}

impl VaultScope {
    pub fn key(&self) -> String {
        match self {
            Self::Personal => "personal".to_owned(),
            Self::Org { org_id, .. } => format!("org:{org_id}"),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum VaultSource {
    #[default]
    Sikemux,
    #[serde(rename_all = "camelCase")]
    OnePassword { reference: String },
}

impl VaultSource {
    pub fn kind_str(&self) -> &'static str {
        match self {
            Self::Sikemux => "sikemux",
            Self::OnePassword { .. } => "1password",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VaultDeliveryMode {
    #[default]
    BlindFill,
    DirectEnv,
    PrivateTempFile,
}

impl VaultDeliveryMode {
    pub fn badge(self) -> &'static str {
        match self {
            Self::BlindFill => "Host blind-fill (never exposed to agents)",
            Self::DirectEnv => "Direct process env (never printed to chat)",
            Self::PrivateTempFile => "Temporary private file (0600, auto-deleted after use)",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VaultFieldKind {
    #[default]
    Text,
    Email,
    Phone,
    Address,
    Secret,
    Password,
    Totp,
    CardNumber,
    Expiry,
    Cvc,
    TaxId,
    AccountNumber,
    Document,
    Url,
}

impl VaultFieldKind {
    pub fn is_inherently_sensitive(self) -> bool {
        matches!(
            self,
            Self::Secret
                | Self::Password
                | Self::Totp
                | Self::CardNumber
                | Self::Cvc
                | Self::TaxId
                | Self::AccountNumber
                | Self::Document
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultFieldInput {
    pub key: String,
    pub label: String,
    #[serde(default)]
    pub kind: VaultFieldKind,
    #[serde(default)]
    pub sensitive: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op_ref: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultFieldSummary {
    pub key: String,
    pub label: String,
    pub kind: VaultFieldKind,
    pub sensitive: bool,
    pub has_value: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub masked_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op_ref: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultFieldRevealed {
    pub key: String,
    pub label: String,
    pub kind: VaultFieldKind,
    pub sensitive: bool,
    pub masked: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op_ref: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultDocumentInput {
    pub filename: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_base64: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultDocumentSummary {
    pub filename: String,
    pub mime_type: String,
    pub byte_len: u64,
    pub sha256_prefix: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultEntrySummary {
    pub id: String,
    pub title: String,
    pub category: VaultCategory,
    pub scope: VaultScope,
    pub source: VaultSource,
    pub delivery_mode: VaultDeliveryMode,
    pub delivery_note: String,
    #[serde(default)]
    pub domains: Vec<String>,
    #[serde(default)]
    pub approved_domains: Vec<String>,
    #[serde(default)]
    pub require_approval: bool,
    #[serde(default)]
    pub fields: Vec<VaultFieldSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document: Option<VaultDocumentSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultEntryDetail {
    pub summary: VaultEntrySummary,
    pub revealed: bool,
    pub fields: Vec<VaultFieldRevealed>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultListParams {
    #[serde(default)]
    pub category: Option<VaultCategory>,
    #[serde(default)]
    pub scope: Option<VaultScope>,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub domain: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultGetParams {
    pub id: String,
    #[serde(default)]
    pub reveal: bool,
    #[serde(default)]
    pub resolve_external: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultUpsertParams {
    #[serde(default)]
    pub id: Option<String>,
    pub title: String,
    pub category: VaultCategory,
    #[serde(default)]
    pub scope: VaultScope,
    #[serde(default)]
    pub source: VaultSource,
    #[serde(default)]
    pub delivery_mode: Option<VaultDeliveryMode>,
    #[serde(default)]
    pub domains: Vec<String>,
    #[serde(default)]
    pub require_approval: bool,
    #[serde(default)]
    pub fields: Vec<VaultFieldInput>,
    #[serde(default)]
    pub document: Option<VaultDocumentInput>,
    #[serde(default)]
    pub subtitle: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultDeleteParams {
    pub id: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VaultAuditStatus {
    Filled,
    Uploaded,
    Approved,
    Denied,
    Failed,
    Created,
    Updated,
    Deleted,
    Revealed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultAuditEntry {
    pub id: String,
    pub entry_id: String,
    pub entry_title: String,
    pub category: VaultCategory,
    #[serde(default)]
    pub field_keys: Vec<String>,
    pub actor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    pub tool: String,
    pub delivery_mode: VaultDeliveryMode,
    pub status: VaultAuditStatus,
    pub at: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultAuditLogParams {
    #[serde(default)]
    pub entry_id: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultRequestedField {
    pub key: String,
    pub label: String,
    #[serde(default)]
    pub kind: VaultFieldKind,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub sensitive: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultPendingRequest {
    pub id: String,
    pub agent_id: String,
    pub agent_name: String,
    pub project: String,
    pub category: VaultCategory,
    pub scope: VaultScope,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    #[serde(default)]
    pub fields: Vec<VaultRequestedField>,
    pub created_at: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultRequestsParams {
    #[serde(default)]
    pub agent_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultRequestSubmitParams {
    pub request_id: String,
    #[serde(default)]
    pub existing_entry_id: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub scope: Option<VaultScope>,
    #[serde(default)]
    pub source: VaultSource,
    #[serde(default)]
    pub domains: Vec<String>,
    #[serde(default)]
    pub require_approval: bool,
    #[serde(default)]
    pub fields: Vec<VaultFieldInput>,
    #[serde(default)]
    pub document: Option<VaultDocumentInput>,
    #[serde(default)]
    pub approve_domain: bool,
    #[serde(default)]
    pub ephemeral: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultStageDocumentParams {
    pub id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultStagedDocument {
    pub path: String,
    pub filename: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultRequestCancelParams {
    pub request_id: String,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultPendingApproval {
    pub id: String,
    pub agent_id: String,
    pub agent_name: String,
    pub entry_id: String,
    pub entry_title: String,
    pub category: VaultCategory,
    pub field_keys: Vec<String>,
    pub domain: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub masked_summary: Option<String>,
    pub requires_biometric: bool,
    pub reason: String,
    pub created_at: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultApprovalsParams {
    #[serde(default)]
    pub agent_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultApprovalAnswerParams {
    pub id: String,
    pub approved: bool,
    #[serde(default)]
    pub remember_domain: bool,
    #[serde(default)]
    pub ephemeral_values: std::collections::BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultResolveBrowserParams {
    pub agent_id: String,
    pub reference: String,
    pub domain: String,
    pub tool: String,
    #[serde(default)]
    pub for_upload: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum VaultResolvedBrowser {
    #[serde(rename_all = "camelCase")]
    Text {
        value: String,
        masked_label: String,
        redaction_tokens: Vec<String>,
        sensitive: bool,
        delivery_mode: VaultDeliveryMode,
    },
    #[serde(rename_all = "camelCase")]
    File {
        staged_path: String,
        display_name: String,
        delivery_mode: VaultDeliveryMode,
    },
}
