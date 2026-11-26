//! Sikemux Agentic Vault: end-to-end encrypted storage for Details, Secrets (native + 1Password
//! `op://` references), Payments, and encrypted Documents, with zero-exposure host blind-fill,
//! explicit approval gates, first-entry card requests, and audit logging.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use sikemux_calls::vault::{
    VaultApprovalAnswerParams, VaultApprovalsParams, VaultAuditEntry, VaultAuditLogParams,
    VaultAuditStatus, VaultDeleteParams, VaultEntryDetail, VaultEntrySummary, VaultGetParams,
    VaultListParams, VaultPendingApproval, VaultPendingRequest, VaultRequestCancelParams,
    VaultRequestSubmitParams, VaultRequestsParams, VaultResolveBrowserParams, VaultResolvedBrowser,
    VaultUpsertParams,
};
use sikemux_process::sync::Locked;

use crate::server::access::Peer;
use crate::server::changed::{ChangedCore, Topic, What};
use crate::server::part::Part;
use crate::server::parts::Parts;
use crate::server::{Core, CoreError, CoreResult};

pub(crate) mod crypto;
mod flow;
mod store;
#[cfg(test)]
mod tests;
mod tools;

pub(crate) use tools::{tool_fill, tool_list, tool_request};

#[derive(Default)]
pub(crate) struct FlowState {
    pub(crate) requests: BTreeMap<String, flow::OpenRequest>,
    pub(crate) approvals: BTreeMap<String, flow::OpenApproval>,
    pub(crate) one_time_grants: Vec<flow::OneTimeGrant>,
}

pub(crate) struct AgentVault {
    pub(crate) store: Mutex<store::VaultStore>,
    pub(crate) flows: Mutex<FlowState>,
}

impl AgentVault {
    pub(crate) fn new(data_dir: Option<&Path>) -> Self {
        Self {
            store: Mutex::new(store::VaultStore::new(data_dir, None)),
            flows: Mutex::new(FlowState::default()),
        }
    }
}

impl Part for AgentVault {}

pub(crate) fn list(core: &Core, params: VaultListParams) -> CoreResult<Vec<VaultEntrySummary>> {
    core.agent_vault()
        .store
        .locked()
        .list(&params)
        .map_err(CoreError::from)
}

pub(crate) async fn get(core: &Arc<Core>, params: VaultGetParams) -> CoreResult<VaultEntryDetail> {
    let mut detail = core
        .agent_vault()
        .store
        .locked()
        .get(&params.id, params.reveal)
        .map_err(CoreError::from)?;
    if params.reveal {
        if params.resolve_external {
            for field in &mut detail.fields {
                if field.value.is_none() {
                    if let Some(op_ref) = field.op_ref.clone() {
                        let resolved = tokio::task::spawn_blocking(move || {
                            sikemux_keychain::reference::resolve(&op_ref)
                        })
                        .await
                        .map_err(|error| CoreError::from(error.to_string()))?
                        .map_err(CoreError::from)?;
                        field.value = Some(resolved);
                    }
                }
            }
        }
        core.agent_vault().store.locked().record_audit(
            &detail.summary.id,
            &detail.summary.title,
            detail.summary.category,
            detail.fields.iter().map(|f| f.key.clone()).collect(),
            "person",
            None,
            "vault.get",
            detail.summary.delivery_mode,
            VaultAuditStatus::Revealed,
        );
    }
    Ok(detail)
}

pub(crate) fn upsert(core: &Core, params: VaultUpsertParams) -> CoreResult<VaultEntrySummary> {
    let summary = core
        .agent_vault()
        .store
        .locked()
        .upsert(params, "person")
        .map_err(CoreError::from)?;
    core.changed(What::Read(Topic::Vault));
    Ok(summary)
}

pub(crate) fn delete(core: &Core, params: VaultDeleteParams) -> CoreResult<()> {
    core.agent_vault()
        .store
        .locked()
        .delete(&params.id, "person")
        .map_err(CoreError::from)?;
    core.changed(What::Read(Topic::Vault));
    Ok(())
}

pub(crate) fn audit_log(
    core: &Core,
    params: VaultAuditLogParams,
) -> CoreResult<Vec<VaultAuditEntry>> {
    core.agent_vault()
        .store
        .locked()
        .audit_log(&params)
        .map_err(CoreError::from)
}

pub(crate) fn requests(core: &Core, params: VaultRequestsParams) -> Vec<VaultPendingRequest> {
    flow::list_requests(core, params)
}

pub(crate) async fn submit_request(
    core: &Arc<Core>,
    params: VaultRequestSubmitParams,
) -> CoreResult<VaultEntrySummary> {
    flow::submit_request(core, params).await
}

pub(crate) async fn cancel_request(
    core: &Arc<Core>,
    params: VaultRequestCancelParams,
) -> CoreResult<()> {
    flow::cancel_request(core, params).await
}

pub(crate) fn approvals(core: &Core, params: VaultApprovalsParams) -> Vec<VaultPendingApproval> {
    flow::list_approvals(core, params)
}

pub(crate) async fn answer_approval(
    core: &Arc<Core>,
    peer: &Peer,
    params: VaultApprovalAnswerParams,
) -> CoreResult<()> {
    flow::answer_approval(core, peer, params).await
}

pub(crate) async fn resolve_for_browser(
    core: &Arc<Core>,
    params: VaultResolveBrowserParams,
) -> CoreResult<VaultResolvedBrowser> {
    flow::resolve_for_browser(core, params).await
}
