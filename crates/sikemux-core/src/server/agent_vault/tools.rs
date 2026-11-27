use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use sikemux_calls::vault::{
    VaultCategory, VaultFieldKind, VaultListParams, VaultPendingRequest, VaultProposedDocument,
    VaultRequestedField, VaultScope,
};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use tokio::sync::oneshot;

use crate::cli::protocol::HarnessRequest;
use crate::server::activity::{Activity, ActivityCore};
use crate::server::changed::{ChangedCore, Topic, What};
use crate::server::parts::Parts;
use crate::server::workspace_state::WorkspaceParts;
use crate::server::Core;

use super::crypto::{normalize_domain, parse_vault_ref};
use super::flow::{OpenRequest, RequestResolution};
use super::store::{guess_mime_type, sanitize_filename, MAX_DOCUMENT_BYTES};

const DEFAULT_REQUEST_WAIT_MS: u64 = 50_000;
const MAX_REQUEST_WAIT_MS: u64 = 55_000;

/// Lists vault entries with strictly safe metadata (IDs, titles, categories, safe field keys/labels,
/// domain hints, source, delivery mode). NEVER returns raw secrets, card numbers/CVCs, or sensitive values.
pub(crate) fn tool_list(core: &Core, request: &HarnessRequest) -> Result<Value, String> {
    let params = &request.params;
    let category = match params.get("category").and_then(Value::as_str) {
        None | Some("") | Some("all") => None,
        Some("details") => Some(VaultCategory::Details),
        Some("secrets") => Some(VaultCategory::Secrets),
        Some("payments") => Some(VaultCategory::Payments),
        Some("documents") => Some(VaultCategory::Documents),
        Some(other) => {
            return Err(format!(
                "category must be details, secrets, payments, or documents, not '{other}'"
            ))
        }
    };
    let query = params
        .get("query")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .map(str::to_owned);
    let domain = params
        .get("domain")
        .and_then(Value::as_str)
        .map(normalize_domain)
        .filter(|d| !d.is_empty());

    let listed = core.agent_vault().store.locked().list(&VaultListParams {
        category,
        scope: None,
        query,
        domain,
    })?;

    let entries: Vec<Value> = listed
        .into_iter()
        .map(|entry| {
            let fields: Vec<Value> = entry
                .fields
                .into_iter()
                .map(|field| {
                    json!({
                        "key": field.key,
                        "label": field.label,
                        "kind": field.kind,
                        "sensitive": field.sensitive,
                        "hasValue": field.has_value,
                        "ref": format!("vault:{}.{}", entry.id, field.key),
                    })
                })
                .collect();
            let mut obj = json!({
                "id": entry.id,
                "ref": format!("vault:{}", entry.id),
                "title": entry.title,
                "category": entry.category.as_str(),
                "scope": entry.scope,
                "source": entry.source.kind_str(),
                "domains": entry.domains,
                "requireApproval": entry.require_approval,
                "deliveryMode": entry.delivery_mode,
                "deliveryNote": entry.delivery_note,
                "fields": fields,
            });
            if let Some(doc) = entry.document {
                if let Some(map) = obj.as_object_mut() {
                    map.insert(
                        "document".to_owned(),
                        json!({
                            "filename": doc.filename,
                            "mimeType": doc.mime_type,
                            "byteLen": doc.byte_len,
                        }),
                    );
                }
            }
            obj
        })
        .collect();

    Ok(json!({
        "entries": entries,
        "count": entries.len(),
        "usage": "Use vault:<id>.<field> in vault_fill, browser_type, or browser_drive (tab.fill / tab.type), and vault:<id> in browser_upload / tab.upload for Documents. Raw values are never returned to agents."
    }))
}

/// Emits a structured host-rendered first-entry card request and waits for the person to fill & save
/// directly into Vault (or decline). Returns only the newly created `vault:<id>` reference + available
/// field keys — NEVER any entered values.
pub(crate) async fn tool_request(
    core: &Arc<Core>,
    request: &HarnessRequest,
) -> Result<Value, String> {
    let agent_id = request
        .agent_id
        .clone()
        .ok_or_else(|| "vault_request needs the calling agent's id".to_owned())?;
    let params = &request.params;
    let category = match params.get("category").and_then(Value::as_str) {
        Some("details") => VaultCategory::Details,
        Some("secrets") => VaultCategory::Secrets,
        Some("payments") => VaultCategory::Payments,
        Some("documents") => VaultCategory::Documents,
        Some(other) => {
            return Err(format!(
                "category must be details, secrets, payments, or documents, not '{other}'"
            ))
        }
        None => {
            return Err("category is required (details, secrets, payments, or documents)".into())
        }
    };
    let title = params
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| "title is required".to_owned())?
        .chars()
        .take(120)
        .collect::<String>();
    let reason = params
        .get("reason")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .map(|r| r.chars().take(400).collect::<String>());
    let domain = params
        .get("domain")
        .and_then(Value::as_str)
        .map(normalize_domain)
        .filter(|d| !d.is_empty());
    let proposed_document = match params.get("filePath") {
        None | Some(Value::Null) => None,
        Some(Value::String(raw)) => {
            if category != VaultCategory::Documents {
                return Err("filePath is only supported when category is 'documents'".into());
            }
            Some(validate_proposed_file(raw)?)
        }
        Some(_) => return Err("filePath must be a string".into()),
    };
    let fields = parse_requested_fields(category, params.get("fields"))?;
    let wait_ms = params
        .get("waitMs")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_REQUEST_WAIT_MS)
        .clamp(1, MAX_REQUEST_WAIT_MS);

    let agent_name = core
        .names()
        .get(&agent_id)
        .map(|named| named.name)
        .unwrap_or_else(|| agent_id.clone());
    let id = format!("vreq_{}", uuid::Uuid::new_v4().simple());
    let pending = VaultPendingRequest {
        id: id.clone(),
        agent_id: agent_id.clone(),
        agent_name,
        project: request.project.clone(),
        category,
        scope: VaultScope::Personal,
        title: title.clone(),
        reason,
        domain,
        fields,
        proposed_document,
        created_at: epoch_ms(),
    };
    let (tx, rx) = oneshot::channel();
    {
        let mut flows = core.agent_vault().flows.locked();
        flows.requests.insert(
            id.clone(),
            OpenRequest {
                view: pending,
                waiter: Some(tx),
            },
        );
    }
    core.note_activity(&agent_id, Activity::NeedsYou);
    core.changed(What::Read(Topic::Vault));
    core.changed(What::Agents);

    match tokio::time::timeout(Duration::from_millis(wait_ms), rx).await {
        Ok(Ok(RequestResolution::Submitted {
            entry_id,
            title,
            category,
            fields,
            source,
        })) => Ok(json!({
            "status": "saved",
            "entryId": entry_id,
            "reference": format!("vault:{entry_id}"),
            "title": title,
            "category": category.as_str(),
            "source": source,
            "fields": fields,
            "note": "Saved directly to Vault by the host. Use vault:<id>.<field> with vault_fill or browser_type; entered values are never exposed."
        })),
        Ok(Ok(RequestResolution::Cancelled { reason })) => Ok(json!({
            "status": "declined",
            "requestId": id,
            "reason": reason,
            "note": "The person declined to save this item to Vault."
        })),
        Ok(Err(_)) => Ok(json!({
            "status": "declined",
            "requestId": id,
        })),
        Err(_) => {
            let mut flows = core.agent_vault().flows.locked();
            if let Some(open) = flows.requests.get_mut(&id) {
                open.waiter = None;
            }
            Ok(json!({
                "status": "pending",
                "requestId": id,
                "note": "Waiting for the person to fill and save the native Vault card. Stop here and wait: Sikemux will message you with the vault:<id> reference once saved."
            }))
        }
    }
}

/// Blind-fills a Vault reference (`vault:<id>.<field>` for text inputs or `vault:<id>` for file uploads)
/// into the active/target browser tab strictly inside the host without exposing plaintext to the agent.
pub(crate) async fn tool_fill(core: &Arc<Core>, request: HarnessRequest) -> Result<Value, String> {
    let params = &request.params;
    let reference = if let Some(r) = params
        .get("ref")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|r| !r.is_empty())
    {
        r.to_owned()
    } else if let Some(entry_id) = params
        .get("entryId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        match params
            .get("fieldKey")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|f| !f.is_empty())
        {
            Some(field_key) => format!("vault:{entry_id}.{field_key}"),
            None => format!("vault:{entry_id}"),
        }
    } else {
        return Err("ref (vault:<id>.<field> or vault:<id>) is required".into());
    };

    let parsed = parse_vault_ref(&reference).ok_or_else(|| {
        format!("invalid vault reference '{reference}'; expected vault:<id>.<field> or vault:<id>")
    })?;
    let entry = core
        .agent_vault()
        .store
        .locked()
        .entry(&parsed.entry_id)?
        .ok_or_else(|| format!("vault entry '{}' was not found", parsed.entry_id))?;

    let is_upload = parsed.field_key.is_none() || entry.category == VaultCategory::Documents;
    let mut forwarded_params = serde_json::Map::new();
    for passthrough in [
        "tabId", "index", "selector", "field", "replace", "submit", "report",
    ] {
        if let Some(val) = params.get(passthrough) {
            forwarded_params.insert(passthrough.to_owned(), val.clone());
        }
    }

    let browser_method = if is_upload {
        forwarded_params.insert("paths".to_owned(), json!([reference]));
        "browser.upload"
    } else {
        if !forwarded_params.contains_key("replace") {
            forwarded_params.insert("replace".to_owned(), json!(true));
        }
        forwarded_params.insert("text".to_owned(), json!(reference));
        "browser.type"
    };

    let browser_request = HarnessRequest {
        id: request.id,
        method: browser_method.to_owned(),
        params: Value::Object(forwarded_params),
        project: request.project,
        agent_id: request.agent_id,
    };
    crate::server::harness::forward(core, browser_request).await
}

fn parse_requested_fields(
    category: VaultCategory,
    raw: Option<&Value>,
) -> Result<Vec<VaultRequestedField>, String> {
    let Some(list) = raw.and_then(Value::as_array) else {
        return Ok(default_requested_fields(category));
    };
    if list.is_empty() {
        return Ok(default_requested_fields(category));
    }
    let mut fields = Vec::with_capacity(list.len());
    for item in list {
        match item {
            Value::String(key) => {
                let key = key.trim();
                if key.is_empty() {
                    continue;
                }
                let kind = infer_field_kind(category, key);
                fields.push(VaultRequestedField {
                    key: key.to_owned(),
                    label: humanize_key(key),
                    kind,
                    required: true,
                    sensitive: kind.is_inherently_sensitive()
                        || matches!(category, VaultCategory::Secrets | VaultCategory::Payments),
                    placeholder: None,
                    value: None,
                });
            }
            Value::Object(map) => {
                let key = map
                    .get("key")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|k| !k.is_empty())
                    .ok_or_else(|| "each requested field needs a non-empty key".to_owned())?;
                let label = map
                    .get("label")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(str::to_owned)
                    .unwrap_or_else(|| humanize_key(key));
                let kind = map
                    .get("kind")
                    .cloned()
                    .and_then(|v| serde_json::from_value::<VaultFieldKind>(v).ok())
                    .unwrap_or_else(|| infer_field_kind(category, key));
                let required = map.get("required").and_then(Value::as_bool).unwrap_or(true);
                let sensitive = map.get("sensitive").and_then(Value::as_bool).unwrap_or(
                    kind.is_inherently_sensitive()
                        || matches!(category, VaultCategory::Secrets | VaultCategory::Payments),
                );
                let placeholder = map
                    .get("placeholder")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                let value = match map.get("value") {
                    None | Some(Value::Null) => None,
                    Some(Value::String(raw_value)) => {
                        let trimmed = raw_value.trim();
                        if trimmed.is_empty() {
                            None
                        } else {
                            validate_prefilled_value(category, key, kind, sensitive)?;
                            Some(trimmed.chars().take(1024).collect())
                        }
                    }
                    Some(_) => {
                        return Err(format!(
                            "prefilled value for field '{key}' must be a string"
                        ))
                    }
                };
                fields.push(VaultRequestedField {
                    key: key.to_owned(),
                    label,
                    kind,
                    required,
                    sensitive,
                    placeholder,
                    value,
                });
            }
            _ => return Err("fields entries must be strings or objects".to_owned()),
        }
    }
    Ok(fields)
}

fn validate_prefilled_value(
    category: VaultCategory,
    key: &str,
    kind: VaultFieldKind,
    sensitive: bool,
) -> Result<(), String> {
    match category {
        VaultCategory::Secrets | VaultCategory::Payments => Err(format!(
            "prefilled value for '{key}' is not allowed for {} requests; let the person enter it directly",
            category.as_str()
        )),
        VaultCategory::Details => {
            if sensitive || kind.is_inherently_sensitive() {
                Err(format!(
                    "prefilled value is not allowed for sensitive field '{key}'"
                ))
            } else {
                Ok(())
            }
        }
        VaultCategory::Documents => {
            if matches!(
                kind,
                VaultFieldKind::Secret
                    | VaultFieldKind::Password
                    | VaultFieldKind::Totp
                    | VaultFieldKind::CardNumber
                    | VaultFieldKind::Cvc
                    | VaultFieldKind::AccountNumber
                    | VaultFieldKind::Document
            ) {
                Err(format!(
                    "prefilled value is not allowed for sensitive field '{key}'"
                ))
            } else {
                Ok(())
            }
        }
    }
}

fn validate_proposed_file(raw: &str) -> Result<VaultProposedDocument, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("filePath cannot be empty".into());
    }
    if trimmed.chars().any(char::is_control) {
        return Err("filePath cannot contain control characters".into());
    }
    let has_tilde = trimmed == "~" || trimmed.starts_with("~/") || trimmed.starts_with("~\\");
    if !has_tilde && !Path::new(trimmed).is_absolute() {
        return Err("filePath must be an absolute path or start with ~/".into());
    }
    let expanded = sikemux_process::paths::expand_home(trimmed);
    if !expanded.is_absolute() {
        return Err("filePath could not be resolved to an absolute path".into());
    }
    let link_meta = std::fs::symlink_metadata(&expanded)
        .map_err(|error| format!("filePath is not accessible: {error}"))?;
    if link_meta.file_type().is_symlink() {
        return Err("filePath cannot be a symbolic link".into());
    }
    if !link_meta.file_type().is_file() {
        return Err("filePath must point to a regular file".into());
    }
    let canonical = sikemux_process::files::real_path(&expanded)
        .map_err(|error| format!("could not resolve filePath: {error}"))?;
    let meta = std::fs::metadata(&canonical)
        .map_err(|error| format!("could not inspect filePath: {error}"))?;
    if !meta.is_file() {
        return Err("filePath must point to a regular file".into());
    }
    if meta.len() == 0 {
        return Err("filePath points to an empty file".into());
    }
    if meta.len() > MAX_DOCUMENT_BYTES as u64 {
        return Err(format!(
            "document exceeds the {} MB vault limit",
            MAX_DOCUMENT_BYTES / (1024 * 1024)
        ));
    }
    std::fs::File::open(&canonical)
        .map_err(|error| format!("filePath could not be opened: {error}"))?;
    let filename = sanitize_filename(
        &canonical
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
    );
    let mime_type = guess_mime_type(&filename).to_owned();
    let display_path = format_display_path(&canonical);
    Ok(VaultProposedDocument {
        source_path: canonical.to_string_lossy().into_owned(),
        display_path,
        filename,
        mime_type,
        byte_len: meta.len(),
    })
}

fn format_display_path(canonical: &Path) -> String {
    if let Some(home) = sikemux_process::paths::home() {
        let home_canonical = sikemux_process::files::real_path(&home).unwrap_or(home.clone());
        for root in [&home_canonical, &home] {
            if let Ok(rel) = canonical.strip_prefix(root) {
                return if rel.as_os_str().is_empty() {
                    "~".to_owned()
                } else {
                    format!("~/{}", rel.to_string_lossy())
                };
            }
        }
    }
    canonical.to_string_lossy().into_owned()
}

fn default_requested_fields(category: VaultCategory) -> Vec<VaultRequestedField> {
    let field = |key: &str, label: &str, kind: VaultFieldKind, required: bool, sensitive: bool| {
        VaultRequestedField {
            key: key.to_owned(),
            label: label.to_owned(),
            kind,
            required,
            sensitive,
            placeholder: None,
            value: None,
        }
    };
    match category {
        VaultCategory::Details => vec![
            field("full_name", "Full name", VaultFieldKind::Text, true, false),
            field("email", "Email", VaultFieldKind::Email, true, false),
            field(
                "address",
                "Street address",
                VaultFieldKind::Address,
                false,
                false,
            ),
        ],
        VaultCategory::Secrets => vec![
            field(
                "username",
                "Username or email",
                VaultFieldKind::Email,
                false,
                false,
            ),
            field("password", "Password", VaultFieldKind::Password, true, true),
            field(
                "totp",
                "2FA / TOTP setup key",
                VaultFieldKind::Totp,
                false,
                true,
            ),
        ],
        VaultCategory::Payments => vec![
            field(
                "cardholder",
                "Name on card",
                VaultFieldKind::Text,
                true,
                false,
            ),
            field(
                "card_number",
                "Card number",
                VaultFieldKind::CardNumber,
                true,
                true,
            ),
            field("expiry", "Expiry", VaultFieldKind::Expiry, true, true),
            field("cvc", "CVC", VaultFieldKind::Cvc, true, true),
            field(
                "billing_address",
                "Billing address",
                VaultFieldKind::Address,
                false,
                false,
            ),
        ],
        VaultCategory::Documents => vec![
            field(
                "document",
                "Document file",
                VaultFieldKind::Document,
                true,
                true,
            ),
            field(
                "document_number",
                "ID number",
                VaultFieldKind::TaxId,
                false,
                true,
            ),
        ],
    }
}

fn infer_field_kind(category: VaultCategory, key: &str) -> VaultFieldKind {
    let lower = key.to_lowercase();
    let tokens: Vec<&str> = lower.split(['_', '-']).filter(|t| !t.is_empty()).collect();
    let has = |needle: &str| tokens.contains(&needle);

    if has("passport")
        || has("pan")
        || has("ssn")
        || has("tax")
        || has("vat")
        || has("gst")
        || has("gstin")
        || has("udyam")
        || has("tin")
        || has("ein")
        || lower == "document_number"
        || lower == "id_number"
        || lower.contains("tax_id")
        || lower.contains("registration_number")
    {
        VaultFieldKind::TaxId
    } else if has("password") || has("passcode") || has("passphrase") || has("pwd") || has("pass") {
        VaultFieldKind::Password
    } else if has("totp") || has("otp") || has("2fa") || has("mfa") {
        VaultFieldKind::Totp
    } else if has("cvc") || has("cvv") {
        VaultFieldKind::Cvc
    } else if has("card") || lower.contains("card_number") {
        VaultFieldKind::CardNumber
    } else if has("exp") || has("expiry") || has("expiration") {
        VaultFieldKind::Expiry
    } else if has("email") {
        VaultFieldKind::Email
    } else if has("phone") || has("mobile") {
        VaultFieldKind::Phone
    } else if has("address") || has("street") {
        VaultFieldKind::Address
    } else if has("account") || has("routing") || has("iban") || has("ifsc") {
        VaultFieldKind::AccountNumber
    } else if has("key") || has("token") || has("secret") {
        VaultFieldKind::Secret
    } else if category == VaultCategory::Documents
        && matches!(lower.as_str(), "document" | "file" | "attachment")
    {
        VaultFieldKind::Document
    } else {
        VaultFieldKind::Text
    }
}

fn humanize_key(key: &str) -> String {
    let mut words = Vec::new();
    for part in key.split(['_', '-']).filter(|p| !p.is_empty()) {
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            let mut word = String::new();
            word.extend(first.to_uppercase());
            word.push_str(chars.as_str());
            words.push(word);
        }
    }
    if words.is_empty() {
        key.to_owned()
    } else {
        words.join(" ")
    }
}
