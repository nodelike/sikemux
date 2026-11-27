use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sikemux_calls::vault::{
    VaultApprovalAnswerParams, VaultApprovalsParams, VaultAuditStatus, VaultCategory,
    VaultDeliveryMode, VaultDocumentInput, VaultEntrySummary, VaultFieldKind, VaultPendingApproval,
    VaultPendingRequest, VaultRequestCancelParams, VaultRequestSubmitParams, VaultRequestsParams,
    VaultResolveBrowserParams, VaultResolvedBrowser, VaultSource, VaultUpsertParams,
};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use tokio::sync::oneshot;

use crate::server::activity::{Activity, ActivityCore};
use crate::server::changed::{ChangedCore, Topic, What};
use crate::server::parts::Parts;
use crate::server::remote_state::RemoteParts;
use crate::server::workspace_state::WorkspaceParts;
use crate::server::{Core, CoreError, CoreResult};

use super::crypto::{
    domain_matches, generate_totp, mask_value, normalize_domain, parse_op_ref, parse_vault_ref,
    redaction_tokens,
};
use super::store::StoredEntry;

const ONE_TIME_GRANT_TTL_MS: u64 = 120_000;
const APPROVAL_WAIT: Duration = Duration::from_secs(50);

#[derive(Clone, Debug)]
pub(crate) enum RequestResolution {
    Submitted {
        entry_id: String,
        title: String,
        category: VaultCategory,
        fields: Vec<String>,
        source: String,
    },
    Cancelled {
        reason: Option<String>,
    },
}

pub(crate) struct OpenRequest {
    pub(crate) view: VaultPendingRequest,
    pub(crate) waiter: Option<oneshot::Sender<RequestResolution>>,
}

#[derive(Clone, Debug)]
pub(crate) struct ApprovalResolution {
    pub(crate) approved: bool,
    pub(crate) ephemeral_values: BTreeMap<String, String>,
}

pub(crate) struct OpenApproval {
    pub(crate) view: VaultPendingApproval,
    pub(crate) waiter: Option<oneshot::Sender<ApprovalResolution>>,
}

#[derive(Clone, Debug)]
pub(crate) struct OneTimeGrant {
    pub(crate) agent_id: String,
    pub(crate) entry_id: String,
    pub(crate) domain: String,
    pub(crate) expires_at: u64,
    pub(crate) ephemeral_values: BTreeMap<String, String>,
}

pub(crate) fn list_requests(core: &Core, params: VaultRequestsParams) -> Vec<VaultPendingRequest> {
    let state = core.agent_vault().flows.locked();
    state
        .requests
        .values()
        .filter(|open| {
            params
                .agent_id
                .as_deref()
                .is_none_or(|id| open.view.agent_id == id)
        })
        .map(|open| open.view.clone())
        .collect()
}

pub(crate) fn list_approvals(
    core: &Core,
    params: VaultApprovalsParams,
) -> Vec<VaultPendingApproval> {
    let state = core.agent_vault().flows.locked();
    state
        .approvals
        .values()
        .filter(|open| {
            params
                .agent_id
                .as_deref()
                .is_none_or(|id| open.view.agent_id == id)
        })
        .map(|open| open.view.clone())
        .collect()
}

pub(crate) async fn submit_request(
    core: &Arc<Core>,
    params: VaultRequestSubmitParams,
) -> CoreResult<VaultEntrySummary> {
    let open = {
        let mut flows = core.agent_vault().flows.locked();
        flows
            .requests
            .remove(&params.request_id)
            .ok_or_else(|| CoreError::from("that vault request is no longer open"))?
    };
    let summary = if let Some(existing_id) = params
        .existing_entry_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        let mut store = core.agent_vault().store.locked();
        if params.approve_domain {
            if let Some(ref domain) = open.view.domain {
                store
                    .approve_domain(existing_id, domain)
                    .map_err(CoreError::from)?;
            }
        }
        store
            .entry(existing_id)
            .map_err(CoreError::from)?
            .ok_or_else(|| CoreError::from(format!("vault entry {existing_id} was not found")))?
            .summary()
    } else {
        let mut domains = params.domains;
        if let Some(ref domain) = open.view.domain {
            let norm = normalize_domain(domain);
            if !norm.is_empty() && !domains.iter().any(|d| normalize_domain(d) == norm) {
                domains.push(norm);
            }
        }
        let title = params
            .title
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| open.view.title.clone());
        let scope = params.scope.unwrap_or_else(|| open.view.scope.clone());
        let document = params.document.or_else(|| {
            open.view
                .proposed_document
                .as_ref()
                .map(|doc| VaultDocumentInput {
                    filename: doc.filename.clone(),
                    mime_type: Some(doc.mime_type.clone()),
                    source_path: Some(doc.source_path.clone()),
                    data_base64: None,
                })
        });
        let upsert = VaultUpsertParams {
            id: None,
            title,
            category: open.view.category,
            scope,
            source: params.source,
            delivery_mode: None,
            domains,
            require_approval: params.require_approval,
            fields: params.fields,
            document,
            subtitle: None,
        };
        let mut store = core.agent_vault().store.locked();
        if params.ephemeral {
            let approved = params
                .approve_domain
                .then_some(open.view.domain.as_deref())
                .flatten();
            store
                .upsert_ephemeral(upsert, approved, ONE_TIME_GRANT_TTL_MS)
                .map_err(CoreError::from)?
        } else {
            let saved = store.upsert(upsert, "person").map_err(CoreError::from)?;
            if params.approve_domain {
                if let Some(ref domain) = open.view.domain {
                    let _ = store.approve_domain(&saved.id, domain);
                }
            }
            saved
        }
    };

    let resolution = RequestResolution::Submitted {
        entry_id: summary.id.clone(),
        title: summary.title.clone(),
        category: summary.category,
        fields: summary.fields.iter().map(|f| f.key.clone()).collect(),
        source: summary.source.kind_str().to_owned(),
    };
    let delivered_inline = open
        .waiter
        .is_some_and(|tx| tx.send(resolution.clone()).is_ok());

    core.changed(What::Read(Topic::Vault));
    core.changed(What::Agents);

    if !delivered_inline {
        let keys = summary
            .fields
            .iter()
            .map(|f| f.key.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let note = format!(
            "The person saved '{}' ({}) to Vault as vault:{}. Available fields: [{}]. Use vault:{}.<field> with vault_fill / browser_type / browser_upload; raw values are never shown.",
            summary.title,
            summary.category.as_str(),
            summary.id,
            keys,
            summary.id
        );
        let _ = crate::server::messages::from_sikemux(core, open.view.agent_id, note).await;
    }
    Ok(summary)
}

pub(crate) async fn cancel_request(
    core: &Arc<Core>,
    params: VaultRequestCancelParams,
) -> CoreResult<()> {
    let open = {
        let mut flows = core.agent_vault().flows.locked();
        flows
            .requests
            .remove(&params.request_id)
            .ok_or_else(|| CoreError::from("that vault request is no longer open"))?
    };
    let resolution = RequestResolution::Cancelled {
        reason: params.reason.clone(),
    };
    let delivered_inline = open.waiter.is_some_and(|tx| tx.send(resolution).is_ok());
    core.changed(What::Read(Topic::Vault));
    core.changed(What::Agents);
    if !delivered_inline {
        let why = params
            .reason
            .as_deref()
            .filter(|r| !r.trim().is_empty())
            .map(|r| format!(" ({r})"))
            .unwrap_or_default();
        let note = format!(
            "The person declined the Vault request for '{}'{why}. Nothing was saved.",
            open.view.title
        );
        let _ = crate::server::messages::from_sikemux(core, open.view.agent_id, note).await;
    }
    Ok(())
}

pub(crate) async fn answer_approval(
    core: &Arc<Core>,
    peer: &crate::server::access::Peer,
    params: VaultApprovalAnswerParams,
) -> CoreResult<()> {
    let requires_biometric = {
        let flows = core.agent_vault().flows.locked();
        flows
            .approvals
            .get(&params.id)
            .map(|open| open.view.requires_biometric)
            .ok_or_else(|| CoreError::from("that vault fill approval is no longer open"))?
    };
    if params.approved && requires_biometric {
        core.remote()
            .sign(
                peer,
                "Approve blind-filling payment card from Sikemux Vault.",
            )
            .await?;
    }
    let open = {
        let mut flows = core.agent_vault().flows.locked();
        flows
            .approvals
            .remove(&params.id)
            .ok_or_else(|| CoreError::from("that vault fill approval is no longer open"))?
    };
    {
        let mut store = core.agent_vault().store.locked();
        if params.approved
            && params.remember_domain
            && open.view.category != VaultCategory::Payments
        {
            let _ = store.approve_domain(&open.view.entry_id, &open.view.domain);
        }
        store.record_audit(
            &open.view.entry_id,
            &open.view.entry_title,
            open.view.category,
            open.view.field_keys.clone(),
            &open.view.agent_name,
            Some(open.view.domain.clone()),
            "vault.approval",
            if open.view.category == VaultCategory::Documents {
                VaultDeliveryMode::PrivateTempFile
            } else {
                VaultDeliveryMode::BlindFill
            },
            if params.approved {
                VaultAuditStatus::Approved
            } else {
                VaultAuditStatus::Denied
            },
        );
    }
    let resolution = ApprovalResolution {
        approved: params.approved,
        ephemeral_values: params.ephemeral_values.clone(),
    };
    let delivered_inline = open.waiter.is_some_and(|tx| tx.send(resolution).is_ok());
    if !delivered_inline && params.approved {
        let mut flows = core.agent_vault().flows.locked();
        flows.one_time_grants.push(OneTimeGrant {
            agent_id: open.view.agent_id.clone(),
            entry_id: open.view.entry_id.clone(),
            domain: normalize_domain(&open.view.domain),
            expires_at: epoch_ms().saturating_add(ONE_TIME_GRANT_TTL_MS),
            ephemeral_values: params.ephemeral_values,
        });
    }
    core.changed(What::Read(Topic::Vault));
    core.changed(What::Agents);
    if !delivered_inline {
        let note = if params.approved {
            format!(
                "The person approved blind-filling '{}' on {}. Retry the blind-fill now.",
                open.view.entry_title, open.view.domain
            )
        } else {
            format!(
                "The person declined blind-filling '{}' on {}. Do not fill it.",
                open.view.entry_title, open.view.domain
            )
        };
        let _ = crate::server::messages::from_sikemux(core, open.view.agent_id, note).await;
    }
    Ok(())
}

pub(crate) fn needs_approval(
    entry: &StoredEntry,
    field_sensitive: bool,
    domain: &str,
) -> Option<(bool, String)> {
    let norm = normalize_domain(domain);
    match entry.category {
        VaultCategory::Payments => Some((
            true,
            format!(
                "Approve payment fill '{}' on {}",
                entry.title,
                if norm.is_empty() { "this page" } else { &norm }
            ),
        )),
        VaultCategory::Secrets => {
            let domain_approved = !norm.is_empty()
                && entry
                    .approved_domains
                    .iter()
                    .any(|d| domain_matches(d, &norm));
            if entry.require_approval || !domain_approved {
                let bound_mismatch = !entry.domains.is_empty()
                    && !norm.is_empty()
                    && !entry.domains.iter().any(|d| domain_matches(d, &norm));
                let reason = if bound_mismatch {
                    format!(
                        "'{}' is bound to [{}], not {}. Approve filling here?",
                        entry.title,
                        entry.domains.join(", "),
                        norm
                    )
                } else {
                    format!(
                        "Approve first-time secret fill '{}' on {}",
                        entry.title,
                        if norm.is_empty() { "this page" } else { &norm }
                    )
                };
                Some((false, reason))
            } else {
                None
            }
        }
        VaultCategory::Documents => {
            let domain_approved = !norm.is_empty()
                && entry
                    .approved_domains
                    .iter()
                    .any(|d| domain_matches(d, &norm));
            if entry.require_approval || !domain_approved {
                Some((
                    false,
                    format!(
                        "Approve uploading encrypted document '{}' to {}",
                        entry.title,
                        if norm.is_empty() { "this page" } else { &norm }
                    ),
                ))
            } else {
                None
            }
        }
        VaultCategory::Details => {
            if entry.require_approval
                || (field_sensitive
                    && (norm.is_empty()
                        || !entry
                            .approved_domains
                            .iter()
                            .any(|d| domain_matches(d, &norm))))
            {
                Some((
                    false,
                    format!(
                        "Approve filling sensitive detail '{}' on {}",
                        entry.title,
                        if norm.is_empty() { "this page" } else { &norm }
                    ),
                ))
            } else {
                None
            }
        }
    }
}

pub(crate) async fn resolve_for_browser(
    core: &Arc<Core>,
    params: VaultResolveBrowserParams,
) -> CoreResult<VaultResolvedBrowser> {
    let parsed = parse_vault_ref(&params.reference).ok_or_else(|| {
        CoreError::from(format!(
            "invalid vault reference '{}'; expected vault:<id>.<field> or vault:<id>",
            params.reference
        ))
    })?;
    let entry = {
        let mut store = core.agent_vault().store.locked();
        store
            .entry(&parsed.entry_id)
            .map_err(CoreError::from)?
            .ok_or_else(|| {
                CoreError::from(format!(
                    "vault entry '{}' does not exist; call vault_list or vault_request",
                    parsed.entry_id
                ))
            })?
    };

    let actor = core
        .names()
        .get(&params.agent_id)
        .map(|named| format!("@{}", named.name))
        .unwrap_or_else(|| params.agent_id.clone());
    let norm_domain = normalize_domain(&params.domain);

    if params.for_upload || entry.category == VaultCategory::Documents {
        if entry.document.is_none() {
            return Err(CoreError::from(format!(
                "vault entry '{}' has no document attached",
                entry.title
            )));
        }
        let _ephemeral = gate_fill(
            core,
            &entry,
            &params.agent_id,
            &actor,
            &norm_domain,
            vec!["document".to_owned()],
            true,
        )
        .await?;
        let staged = {
            let mut store = core.agent_vault().store.locked();
            let result = store.stage_document_file(&entry.id);
            match &result {
                Ok(_) => {
                    store.mark_used(&entry.id);
                    store.record_audit(
                        &entry.id,
                        &entry.title,
                        entry.category,
                        vec!["document".to_owned()],
                        &actor,
                        Some(norm_domain),
                        &params.tool,
                        VaultDeliveryMode::PrivateTempFile,
                        VaultAuditStatus::Uploaded,
                    );
                }
                Err(_) => {
                    store.record_audit(
                        &entry.id,
                        &entry.title,
                        entry.category,
                        vec!["document".to_owned()],
                        &actor,
                        Some(norm_domain),
                        &params.tool,
                        VaultDeliveryMode::PrivateTempFile,
                        VaultAuditStatus::Failed,
                    );
                }
            }
            result.map_err(CoreError::from)?
        };
        core.changed(What::Read(Topic::Vault));
        return Ok(VaultResolvedBrowser::File {
            staged_path: staged.0.to_string_lossy().into_owned(),
            display_name: staged.1,
            delivery_mode: VaultDeliveryMode::PrivateTempFile,
        });
    }

    let field_key = parsed.field_key.ok_or_else(|| {
        CoreError::from(format!(
            "vault reference '{}' must specify a field (vault:<id>.<field>)",
            params.reference
        ))
    })?;
    let field = entry
        .fields
        .iter()
        .find(|f| f.key == field_key)
        .cloned()
        .ok_or_else(|| {
            let keys: Vec<&str> = entry.fields.iter().map(|f| f.key.as_str()).collect();
            CoreError::from(format!(
                "vault entry '{}' has no field '{}'; available fields: [{}]",
                entry.title,
                field_key,
                keys.join(", ")
            ))
        })?;

    let ephemeral = gate_fill(
        core,
        &entry,
        &params.agent_id,
        &actor,
        &norm_domain,
        vec![field.key.clone()],
        field.sensitive,
    )
    .await?;

    let resolved_raw = if let Some(val) = ephemeral.get(&field.key) {
        val.clone()
    } else if let Some(op_ref) = field.op_ref.clone().or_else(|| match &entry.source {
        VaultSource::OnePassword { reference } => Some(reference.clone()),
        VaultSource::Sikemux => None,
    }) {
        parse_op_ref(&op_ref).map_err(CoreError::from)?;
        let resolved =
            tokio::task::spawn_blocking(move || sikemux_keychain::reference::resolve(&op_ref))
                .await
                .map_err(|error| CoreError::from(error.to_string()))?
                .map_err(CoreError::from)?;
        resolved
    } else if let Some(val) = field.value.clone() {
        val
    } else {
        return Err(CoreError::from(format!(
            "field '{}' in '{}' has no saved value",
            field.key, entry.title
        )));
    };

    let final_value = if field.kind == VaultFieldKind::Totp {
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        generate_totp(&resolved_raw, now_secs).map_err(CoreError::from)?
    } else {
        resolved_raw
    };

    let masked_label = if field.kind == VaultFieldKind::Totp {
        "•••••• (2FA)".to_owned()
    } else {
        mask_value(field.kind, field.sensitive, &final_value)
    };
    let tokens = if field.sensitive || field.kind.is_inherently_sensitive() {
        redaction_tokens(&final_value, field.kind)
    } else {
        Vec::new()
    };

    {
        let mut store = core.agent_vault().store.locked();
        store.mark_used(&entry.id);
        store.record_audit(
            &entry.id,
            &entry.title,
            entry.category,
            vec![field.key.clone()],
            &actor,
            Some(norm_domain),
            &params.tool,
            entry.delivery_mode,
            VaultAuditStatus::Filled,
        );
    }
    core.changed(What::Read(Topic::Vault));

    Ok(VaultResolvedBrowser::Text {
        value: final_value,
        masked_label,
        redaction_tokens: tokens,
        sensitive: field.sensitive || field.kind.is_inherently_sensitive(),
        delivery_mode: entry.delivery_mode,
    })
}

async fn gate_fill(
    core: &Arc<Core>,
    entry: &StoredEntry,
    agent_id: &str,
    actor: &str,
    norm_domain: &str,
    field_keys: Vec<String>,
    field_sensitive: bool,
) -> CoreResult<BTreeMap<String, String>> {
    let Some((requires_biometric, reason)) = needs_approval(entry, field_sensitive, norm_domain)
    else {
        return Ok(BTreeMap::new());
    };

    // Consume an existing one-time grant if the person approved earlier after a timeout.
    {
        let mut flows = core.agent_vault().flows.locked();
        let now = epoch_ms();
        flows.one_time_grants.retain(|grant| grant.expires_at > now);
        if let Some(pos) = flows.one_time_grants.iter().position(|grant| {
            grant.agent_id == agent_id && grant.entry_id == entry.id && grant.domain == norm_domain
        }) {
            let grant = flows.one_time_grants.remove(pos);
            return Ok(grant.ephemeral_values);
        }
    }

    let id = format!("vap_{}", uuid::Uuid::new_v4().simple());
    let (tx, rx) = oneshot::channel();
    let pending = VaultPendingApproval {
        id: id.clone(),
        agent_id: agent_id.to_owned(),
        agent_name: actor.to_owned(),
        entry_id: entry.id.clone(),
        entry_title: entry.title.clone(),
        category: entry.category,
        field_keys,
        domain: norm_domain.to_owned(),
        masked_summary: entry.subtitle.clone(),
        requires_biometric,
        reason: reason.clone(),
        created_at: epoch_ms(),
    };
    {
        let mut flows = core.agent_vault().flows.locked();
        flows.approvals.insert(
            id.clone(),
            OpenApproval {
                view: pending,
                waiter: Some(tx),
            },
        );
    }
    core.note_activity(agent_id, Activity::NeedsYou);
    core.changed(What::Read(Topic::Vault));
    core.changed(What::Agents);

    match tokio::time::timeout(APPROVAL_WAIT, rx).await {
        Ok(Ok(resolution)) if resolution.approved => Ok(resolution.ephemeral_values),
        Ok(Ok(_)) => Err(CoreError::from(format!(
            "The person declined filling '{}' on {}.",
            entry.title,
            if norm_domain.is_empty() {
                "this page"
            } else {
                norm_domain
            }
        ))),
        Ok(Err(_)) => Err(CoreError::from("vault approval was cancelled")),
        Err(_) => {
            let mut flows = core.agent_vault().flows.locked();
            if let Some(open) = flows.approvals.get_mut(&id) {
                open.waiter = None;
            }
            Err(CoreError::from(format!(
                "Waits for the person to approve filling '{}' on {} (approvalId: {id}). Stop here and wait for their decision.",
                entry.title,
                if norm_domain.is_empty() {
                    "this page"
                } else {
                    norm_domain
                }
            )))
        }
    }
}
