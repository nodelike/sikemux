use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use sikemux_calls::vault::{
    VaultAuditEntry, VaultAuditLogParams, VaultAuditStatus, VaultCategory, VaultDeliveryMode,
    VaultDocumentInput, VaultDocumentSummary, VaultEntryDetail, VaultEntrySummary, VaultFieldInput,
    VaultFieldKind, VaultFieldRevealed, VaultFieldSummary, VaultListParams, VaultScope,
    VaultSource, VaultUpsertParams,
};
use sikemux_process::clock::epoch_ms;

use super::crypto::{
    domain_matches, mask_value, normalize_domain, open, parse_op_ref, seal, sha256_prefix,
    valid_entry_id, Key,
};

const VAULT_DIR: &str = "agent-vault";
const ENTRIES_FILE: &str = "entries.enc";
const AUDIT_FILE: &str = "audit.json";
const DOCS_DIR: &str = "docs";
const STAGING_DIR: &str = "staging";
const ENTRIES_AAD: &str = "sikemux-agent-vault-entries|v1";
const MAX_AUDIT_ENTRIES: usize = 500;
const MAX_DOCUMENT_BYTES: usize = 25 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StoredField {
    pub(crate) key: String,
    pub(crate) label: String,
    pub(crate) kind: VaultFieldKind,
    pub(crate) sensitive: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) op_ref: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StoredEntry {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) category: VaultCategory,
    pub(crate) scope: VaultScope,
    pub(crate) source: VaultSource,
    pub(crate) delivery_mode: VaultDeliveryMode,
    #[serde(default)]
    pub(crate) domains: Vec<String>,
    #[serde(default)]
    pub(crate) approved_domains: Vec<String>,
    #[serde(default)]
    pub(crate) require_approval: bool,
    #[serde(default)]
    pub(crate) fields: Vec<StoredField>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) document: Option<VaultDocumentSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) subtitle: Option<String>,
    pub(crate) created_at: u64,
    pub(crate) updated_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) last_used_at: Option<u64>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EntriesFile {
    entries: BTreeMap<String, StoredEntry>,
}

pub(crate) struct VaultStore {
    root: Option<PathBuf>,
    key: Option<Key>,
    entries: BTreeMap<String, StoredEntry>,
    in_memory_docs: BTreeMap<String, Vec<u8>>,
    audit: Vec<VaultAuditEntry>,
    loaded: bool,
}

impl VaultStore {
    pub(crate) fn new(data_dir: Option<&Path>, key: Option<Key>) -> Self {
        Self {
            root: data_dir.map(|dir| dir.join(VAULT_DIR)),
            key,
            entries: BTreeMap::new(),
            in_memory_docs: BTreeMap::new(),
            audit: Vec::new(),
            loaded: false,
        }
    }

    fn ensure_key(&mut self) -> Result<Key, String> {
        if let Some(key) = self.key {
            return Ok(key);
        }
        let key = if self.root.is_some() {
            super::crypto::load_or_create_master_key()?
        } else {
            super::crypto::random_key()?
        };
        self.key = Some(key);
        Ok(key)
    }

    pub(crate) fn ensure_loaded(&mut self) -> Result<(), String> {
        if self.loaded {
            return Ok(());
        }
        let Some(root) = self.root.clone() else {
            self.loaded = true;
            return Ok(());
        };
        let entries_path = root.join(ENTRIES_FILE);
        if entries_path.is_file() {
            let key = self.ensure_key()?;
            let sealed = std::fs::read(&entries_path)
                .map_err(|error| format!("could not read Agentic Vault: {error}"))?;
            let plain = open(&key, ENTRIES_AAD, &sealed)?;
            let parsed: EntriesFile = serde_json::from_slice(&plain)
                .map_err(|error| format!("could not parse Agentic Vault: {error}"))?;
            self.entries = parsed.entries;
        }
        let audit_path = root.join(AUDIT_FILE);
        if audit_path.is_file() {
            if let Ok(bytes) = std::fs::read(&audit_path) {
                if let Ok(audit) = serde_json::from_slice::<Vec<VaultAuditEntry>>(&bytes) {
                    self.audit = audit;
                }
            }
        }
        self.loaded = true;
        Ok(())
    }

    fn persist_entries(&mut self) -> Result<(), String> {
        let Some(root) = self.root.clone() else {
            return Ok(());
        };
        let key = self.ensure_key()?;
        std::fs::create_dir_all(&root)
            .map_err(|error| format!("could not create Agentic Vault folder: {error}"))?;
        let payload = serde_json::to_vec(&EntriesFile {
            entries: self.entries.clone(),
        })
        .map_err(|error| format!("could not encode Agentic Vault: {error}"))?;
        let sealed = seal(&key, ENTRIES_AAD, &payload)?;
        sikemux_process::files::write_atomic(&root.join(ENTRIES_FILE), &sealed, true)
            .map_err(|error| format!("could not save Agentic Vault: {error}"))?;
        Ok(())
    }

    fn persist_audit(&self) {
        let Some(root) = &self.root else {
            return;
        };
        if std::fs::create_dir_all(root).is_err() {
            return;
        }
        if let Ok(bytes) = serde_json::to_vec_pretty(&self.audit) {
            let _ = sikemux_process::files::write_atomic(&root.join(AUDIT_FILE), &bytes, true);
        }
    }

    fn doc_aad(id: &str) -> String {
        format!("sikemux-agent-vault-doc|v1|{id}")
    }

    fn doc_path(&self, id: &str) -> Option<PathBuf> {
        self.root
            .as_ref()
            .map(|root| root.join(DOCS_DIR).join(format!("{id}.enc")))
    }

    fn save_document_bytes(&mut self, id: &str, bytes: &[u8]) -> Result<(), String> {
        if bytes.len() > MAX_DOCUMENT_BYTES {
            return Err(format!(
                "document exceeds the {} MB vault limit",
                MAX_DOCUMENT_BYTES / (1024 * 1024)
            ));
        }
        let key = self.ensure_key()?;
        let sealed = seal(&key, &Self::doc_aad(id), bytes)?;
        if let Some(path) = self.doc_path(id) {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| format!("could not create vault documents folder: {error}"))?;
            }
            sikemux_process::files::write_atomic(&path, &sealed, true)
                .map_err(|error| format!("could not store encrypted document: {error}"))?;
        } else {
            self.in_memory_docs.insert(id.to_owned(), sealed);
        }
        Ok(())
    }

    pub(crate) fn read_document_bytes(&mut self, id: &str) -> Result<Vec<u8>, String> {
        self.ensure_loaded()?;
        let key = self.ensure_key()?;
        let sealed = if let Some(path) = self.doc_path(id) {
            std::fs::read(&path)
                .map_err(|error| format!("could not read encrypted document: {error}"))?
        } else {
            self.in_memory_docs
                .get(id)
                .cloned()
                .ok_or_else(|| "vault document blob not found".to_owned())?
        };
        open(&key, &Self::doc_aad(id), &sealed)
    }

    pub(crate) fn stage_document_file(&mut self, id: &str) -> Result<(PathBuf, String), String> {
        self.ensure_loaded()?;
        let doc = self
            .entries
            .get(id)
            .and_then(|entry| entry.document.clone())
            .ok_or_else(|| format!("vault entry {id} does not hold a document"))?;
        let bytes = self.read_document_bytes(id)?;
        let safe_name = sanitize_filename(&doc.filename);
        let stage_base = self
            .root
            .as_ref()
            .map(|root| root.join(STAGING_DIR))
            .unwrap_or_else(|| std::env::temp_dir().join("sikemux-agent-vault-staging"));
        let nonce_dir = stage_base.join(uuid::Uuid::new_v4().simple().to_string());
        std::fs::create_dir_all(&nonce_dir).map_err(|error| {
            format!("could not create private document staging folder: {error}")
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&nonce_dir, std::fs::Permissions::from_mode(0o700));
        }
        let staged = nonce_dir.join(&safe_name);
        sikemux_process::files::write_atomic(&staged, &bytes, true)
            .map_err(|error| format!("could not stage private document: {error}"))?;
        Ok((staged, safe_name))
    }

    pub(crate) fn list(
        &mut self,
        params: &VaultListParams,
    ) -> Result<Vec<VaultEntrySummary>, String> {
        self.ensure_loaded()?;
        let query = params
            .query
            .as_deref()
            .map(str::trim)
            .filter(|q| !q.is_empty())
            .map(str::to_lowercase);
        let domain = params
            .domain
            .as_deref()
            .map(normalize_domain)
            .filter(|d| !d.is_empty());
        let mut out: Vec<VaultEntrySummary> = self
            .entries
            .values()
            .filter(|entry| params.category.is_none_or(|cat| entry.category == cat))
            .filter(|entry| {
                params
                    .scope
                    .as_ref()
                    .is_none_or(|scope| entry.scope.key() == scope.key())
            })
            .filter(|entry| {
                let Some(ref target) = domain else {
                    return true;
                };
                entry.domains.is_empty()
                    || entry
                        .domains
                        .iter()
                        .any(|pattern| domain_matches(pattern, target))
            })
            .filter(|entry| {
                let Some(ref q) = query else {
                    return true;
                };
                entry.title.to_lowercase().contains(q)
                    || entry
                        .subtitle
                        .as_deref()
                        .is_some_and(|s| s.to_lowercase().contains(q))
                    || entry.domains.iter().any(|d| d.to_lowercase().contains(q))
                    || entry.fields.iter().any(|f| {
                        f.key.to_lowercase().contains(q) || f.label.to_lowercase().contains(q)
                    })
            })
            .map(StoredEntry::summary)
            .collect();
        out.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.title.cmp(&b.title))
        });
        Ok(out)
    }

    pub(crate) fn entry(&mut self, id: &str) -> Result<Option<StoredEntry>, String> {
        self.ensure_loaded()?;
        Ok(self.entries.get(id).cloned())
    }

    pub(crate) fn get(&mut self, id: &str, reveal: bool) -> Result<VaultEntryDetail, String> {
        self.ensure_loaded()?;
        let entry = self
            .entries
            .get(id)
            .cloned()
            .ok_or_else(|| format!("vault entry {id} was not found"))?;
        let summary = entry.summary();
        let fields = entry
            .fields
            .iter()
            .map(|field| {
                let masked = field
                    .value
                    .as_deref()
                    .map(|val| mask_value(field.kind, field.sensitive, val))
                    .or_else(|| field.op_ref.as_ref().map(|_| "1Password".to_owned()))
                    .unwrap_or_default();
                let value = if reveal || (!field.sensitive && !field.kind.is_inherently_sensitive())
                {
                    field.value.clone()
                } else {
                    None
                };
                VaultFieldRevealed {
                    key: field.key.clone(),
                    label: field.label.clone(),
                    kind: field.kind,
                    sensitive: field.sensitive,
                    masked,
                    value,
                    op_ref: field.op_ref.clone(),
                }
            })
            .collect();
        Ok(VaultEntryDetail {
            summary,
            revealed: reveal,
            fields,
        })
    }

    pub(crate) fn upsert(
        &mut self,
        params: VaultUpsertParams,
        actor: &str,
    ) -> Result<VaultEntrySummary, String> {
        self.ensure_loaded()?;
        let title = params.title.trim();
        if title.is_empty() {
            return Err("vault entry title is required".to_owned());
        }
        if let VaultSource::OnePassword { ref reference } = params.source {
            parse_op_ref(reference)?;
        }
        let now = epoch_ms();
        let (id, existing) = match params.id {
            Some(id) => {
                let trimmed = id.trim().to_owned();
                if !valid_entry_id(&trimmed) {
                    return Err("vault entry id must be letters, digits, - or _".to_owned());
                }
                let prev = self.entries.get(&trimmed).cloned();
                (trimmed, prev)
            }
            None => (format!("vlt_{}", uuid::Uuid::new_v4().simple()), None),
        };
        let is_new = existing.is_none();
        let fields = build_fields(params.category, params.fields, existing.as_ref())?;
        let document = match params.document {
            Some(doc_input) => {
                let (bytes, doc_summary) = read_document_input(doc_input)?;
                self.save_document_bytes(&id, &bytes)?;
                Some(doc_summary)
            }
            None => existing.as_ref().and_then(|e| e.document.clone()),
        };
        if params.category == VaultCategory::Documents && document.is_none() {
            return Err("a Documents vault entry requires a document attachment".to_owned());
        }
        let domains = normalize_domains(params.domains);
        let delivery_mode = params.delivery_mode.unwrap_or(match params.category {
            VaultCategory::Documents => VaultDeliveryMode::PrivateTempFile,
            _ => VaultDeliveryMode::BlindFill,
        });
        let approved_domains = existing
            .as_ref()
            .map(|e| e.approved_domains.clone())
            .unwrap_or_default();
        let created_at = existing.as_ref().map_or(now, |e| e.created_at);
        let last_used_at = existing.as_ref().and_then(|e| e.last_used_at);
        let require_approval =
            params.require_approval || params.category == VaultCategory::Payments;
        let subtitle = params
            .subtitle
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                default_subtitle(params.category, &fields, document.as_ref(), &params.source)
            });

        let entry = StoredEntry {
            id: id.clone(),
            title: title.to_owned(),
            category: params.category,
            scope: params.scope,
            source: params.source,
            delivery_mode,
            domains,
            approved_domains,
            require_approval,
            fields,
            document,
            subtitle,
            created_at,
            updated_at: now,
            last_used_at,
        };
        let summary = entry.summary();
        let field_keys: Vec<String> = entry.fields.iter().map(|f| f.key.clone()).collect();
        self.entries.insert(id.clone(), entry);
        self.persist_entries()?;
        self.record_audit_inner(
            &summary.id,
            &summary.title,
            summary.category,
            field_keys,
            actor,
            None,
            "vault.upsert",
            summary.delivery_mode,
            if is_new {
                VaultAuditStatus::Created
            } else {
                VaultAuditStatus::Updated
            },
        );
        Ok(summary)
    }

    pub(crate) fn delete(&mut self, id: &str, actor: &str) -> Result<(), String> {
        self.ensure_loaded()?;
        let removed = self
            .entries
            .remove(id)
            .ok_or_else(|| format!("vault entry {id} was not found"))?;
        if let Some(path) = self.doc_path(id) {
            let _ = std::fs::remove_file(path);
        }
        self.in_memory_docs.remove(id);
        self.persist_entries()?;
        self.record_audit_inner(
            &removed.id,
            &removed.title,
            removed.category,
            Vec::new(),
            actor,
            None,
            "vault.delete",
            removed.delivery_mode,
            VaultAuditStatus::Deleted,
        );
        Ok(())
    }

    pub(crate) fn approve_domain(&mut self, id: &str, domain: &str) -> Result<(), String> {
        self.ensure_loaded()?;
        let normalized = normalize_domain(domain);
        if normalized.is_empty() {
            return Ok(());
        }
        let Some(entry) = self.entries.get_mut(id) else {
            return Ok(());
        };
        if entry.category == VaultCategory::Payments {
            return Ok(());
        }
        if !entry.approved_domains.contains(&normalized) {
            entry.approved_domains.push(normalized.clone());
        }
        if !entry.domains.is_empty()
            && !entry
                .domains
                .iter()
                .any(|pattern| domain_matches(pattern, &normalized))
        {
            entry.domains.push(normalized);
        }
        entry.updated_at = epoch_ms();
        self.persist_entries()
    }

    pub(crate) fn mark_used(&mut self, id: &str) {
        if let Some(entry) = self.entries.get_mut(id) {
            entry.last_used_at = Some(epoch_ms());
            let _ = self.persist_entries();
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn record_audit(
        &mut self,
        entry_id: &str,
        entry_title: &str,
        category: VaultCategory,
        field_keys: Vec<String>,
        actor: &str,
        domain: Option<String>,
        tool: &str,
        delivery_mode: VaultDeliveryMode,
        status: VaultAuditStatus,
    ) {
        let _ = self.ensure_loaded();
        self.record_audit_inner(
            entry_id,
            entry_title,
            category,
            field_keys,
            actor,
            domain,
            tool,
            delivery_mode,
            status,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn record_audit_inner(
        &mut self,
        entry_id: &str,
        entry_title: &str,
        category: VaultCategory,
        field_keys: Vec<String>,
        actor: &str,
        domain: Option<String>,
        tool: &str,
        delivery_mode: VaultDeliveryMode,
        status: VaultAuditStatus,
    ) {
        let entry = VaultAuditEntry {
            id: format!("va_{}", uuid::Uuid::new_v4().simple()),
            entry_id: entry_id.to_owned(),
            entry_title: entry_title.to_owned(),
            category,
            field_keys,
            actor: actor.to_owned(),
            domain: domain
                .map(|d| normalize_domain(&d))
                .filter(|d| !d.is_empty()),
            tool: tool.to_owned(),
            delivery_mode,
            status,
            at: epoch_ms(),
        };
        self.audit.push(entry);
        if self.audit.len() > MAX_AUDIT_ENTRIES {
            let drain = self.audit.len() - MAX_AUDIT_ENTRIES;
            self.audit.drain(..drain);
        }
        self.persist_audit();
    }

    pub(crate) fn audit_log(
        &mut self,
        params: &VaultAuditLogParams,
    ) -> Result<Vec<VaultAuditEntry>, String> {
        self.ensure_loaded()?;
        let limit = params.limit.unwrap_or(100).min(MAX_AUDIT_ENTRIES);
        let mut entries: Vec<VaultAuditEntry> = self
            .audit
            .iter()
            .rev()
            .filter(|row| {
                params
                    .entry_id
                    .as_deref()
                    .is_none_or(|id| row.entry_id == id)
            })
            .take(limit)
            .cloned()
            .collect();
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.at));
        Ok(entries)
    }
}

impl StoredEntry {
    pub(crate) fn summary(&self) -> VaultEntrySummary {
        VaultEntrySummary {
            id: self.id.clone(),
            title: self.title.clone(),
            category: self.category,
            scope: self.scope.clone(),
            source: self.source.clone(),
            delivery_mode: self.delivery_mode,
            delivery_note: self.delivery_mode.badge().to_owned(),
            domains: self.domains.clone(),
            approved_domains: self.approved_domains.clone(),
            require_approval: self.require_approval,
            fields: self
                .fields
                .iter()
                .map(|field| VaultFieldSummary {
                    key: field.key.clone(),
                    label: field.label.clone(),
                    kind: field.kind,
                    sensitive: field.sensitive,
                    has_value: field.value.is_some() || field.op_ref.is_some(),
                    masked_hint: field
                        .value
                        .as_deref()
                        .map(|val| mask_value(field.kind, field.sensitive, val))
                        .or_else(|| field.op_ref.as_ref().map(|_| "1Password".to_owned())),
                    op_ref: field.op_ref.clone(),
                })
                .collect(),
            document: self.document.clone(),
            subtitle: self.subtitle.clone(),
            created_at: self.created_at,
            updated_at: self.updated_at,
            last_used_at: self.last_used_at,
        }
    }
}

fn build_fields(
    category: VaultCategory,
    inputs: Vec<VaultFieldInput>,
    existing: Option<&StoredEntry>,
) -> Result<Vec<StoredField>, String> {
    let mut fields = Vec::with_capacity(inputs.len());
    for input in inputs {
        let key = sanitize_field_key(&input.key)?;
        let label = if input.label.trim().is_empty() {
            key.clone()
        } else {
            input.label.trim().to_owned()
        };
        let op_ref = input
            .op_ref
            .map(|r| r.trim().to_owned())
            .filter(|r| !r.is_empty());
        if let Some(ref reference) = op_ref {
            parse_op_ref(reference)?;
        }
        let sensitive = input.sensitive
            || input.kind.is_inherently_sensitive()
            || matches!(category, VaultCategory::Secrets | VaultCategory::Payments);
        let value = match input.value {
            Some(val) if !val.is_empty() => Some(val),
            Some(_) => None,
            None => existing
                .and_then(|prev| prev.fields.iter().find(|f| f.key == key))
                .and_then(|prev| prev.value.clone()),
        };
        fields.push(StoredField {
            key,
            label,
            kind: input.kind,
            sensitive,
            value,
            op_ref,
        });
    }
    Ok(fields)
}

fn sanitize_field_key(raw: &str) -> Result<String, String> {
    let key = raw.trim();
    if key.is_empty() || key.len() > 128 {
        return Err("field key must be 1 to 128 characters".to_owned());
    }
    if !key
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
    {
        return Err(format!(
            "field key '{key}' may only contain ASCII letters, digits, _ or -"
        ));
    }
    Ok(key.to_owned())
}

fn normalize_domains(domains: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    for raw in domains {
        let normalized = normalize_domain(&raw);
        if !normalized.is_empty() && !out.contains(&normalized) {
            out.push(normalized);
        }
    }
    out
}

fn read_document_input(
    input: VaultDocumentInput,
) -> Result<(Vec<u8>, VaultDocumentSummary), String> {
    let filename = sanitize_filename(&input.filename);
    let bytes = match (input.data_base64, input.source_path) {
        (Some(encoded), _) => BASE64_STANDARD
            .decode(encoded.trim())
            .map_err(|error| format!("invalid base64 document payload: {error}"))?,
        (None, Some(path)) => std::fs::read(Path::new(path.trim()))
            .map_err(|error| format!("could not read document at {path}: {error}"))?,
        (None, None) => return Err("document requires either sourcePath or dataBase64".to_owned()),
    };
    if bytes.is_empty() {
        return Err("document file is empty".to_owned());
    }
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err(format!(
            "document exceeds the {} MB vault limit",
            MAX_DOCUMENT_BYTES / (1024 * 1024)
        ));
    }
    let mime_type = input
        .mime_type
        .map(|m| m.trim().to_owned())
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| guess_mime_type(&filename).to_owned());
    let summary = VaultDocumentSummary {
        filename,
        mime_type,
        byte_len: bytes.len() as u64,
        sha256_prefix: sha256_prefix(&bytes),
    };
    Ok((bytes, summary))
}

fn sanitize_filename(raw: &str) -> String {
    let base = Path::new(raw.trim())
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let cleaned: String = base
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '/' | '\\' | '\0'))
        .take(160)
        .collect();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        "document.bin".to_owned()
    } else {
        cleaned
    }
}

fn guess_mime_type(filename: &str) -> &'static str {
    let lower = filename.to_lowercase();
    if lower.ends_with(".pdf") {
        "application/pdf"
    } else if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        "image/jpeg"
    } else if lower.ends_with(".webp") {
        "image/webp"
    } else if lower.ends_with(".txt") {
        "text/plain"
    } else if lower.ends_with(".json") {
        "application/json"
    } else {
        "application/octet-stream"
    }
}

fn default_subtitle(
    category: VaultCategory,
    fields: &[StoredField],
    document: Option<&VaultDocumentSummary>,
    source: &VaultSource,
) -> Option<String> {
    if let VaultSource::OnePassword { .. } = source {
        return Some("1Password reference".to_owned());
    }
    match category {
        VaultCategory::Payments => {
            let masked = fields
                .iter()
                .find(|f| {
                    f.kind == VaultFieldKind::CardNumber || f.kind == VaultFieldKind::AccountNumber
                })
                .and_then(|f| f.value.as_deref().map(|v| mask_value(f.kind, true, v)));
            masked.or_else(|| Some("Payment method".to_owned()))
        }
        VaultCategory::Documents => document.map(|doc| {
            let kb = (doc.byte_len / 1024).max(1);
            format!("{} · {} KB · encrypted", doc.filename, kb)
        }),
        VaultCategory::Secrets => {
            let user = fields
                .iter()
                .find(|f| {
                    matches!(f.kind, VaultFieldKind::Email)
                        || f.key == "username"
                        || f.key == "email"
                })
                .and_then(|f| f.value.as_deref())
                .map(|v| mask_value(VaultFieldKind::Email, true, v));
            user.or_else(|| Some(format!("{} fields · sealed", fields.len())))
        }
        VaultCategory::Details => Some(format!("{} fields", fields.len())),
    }
}
