//! Language servers for the editor: one per project and language, spoken to
//! as JSON-RPC over its stdio. Diagnostics a server publishes go to the core,
//! which keeps each file's latest and tells clients it moved.

mod commands;
mod diagnostics;
mod discovery;
mod documents;
mod held;
mod limits;
mod process;
mod protocol;
mod registry;
mod server;
mod transport;
mod types;

use crate::server::changed::What;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use serde_json::{json, Value};
use sikemux_process::sync::Locked;
use sikemux_wire::protocol::ErrorKind;

use commands::*;
pub(crate) use held::Carried;
pub(crate) use registry::drain_all;
pub use types::*;

use super::changed::Topic;
use super::connection::ClientId;
use super::{Core, CoreResult};

pub(crate) type Publish = Arc<dyn Fn(LspDiagnosticsPayload) + Send + Sync>;

/// What went wrong, and which language and program when a server is not installed.
#[derive(Debug)]
pub(crate) struct LspError {
    message: String,
    missing: Option<Value>,
}

pub(crate) type LspResult<T> = Result<T, LspError>;

impl LspError {
    pub(crate) fn new(message: String) -> Self {
        Self {
            message: format!("lsp: {message}"),
            missing: None,
        }
    }

    pub(crate) fn missing(language: &str, bin: &str) -> Self {
        Self {
            message: format!("lsp server `{bin}` for {language} not found"),
            missing: Some(
                json!({ "language": language, "bin": bin, "install": discovery::install_of(bin) }),
            ),
        }
    }
}

impl std::fmt::Display for LspError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<std::io::Error> for LspError {
    fn from(error: std::io::Error) -> Self {
        Self::new(error.to_string())
    }
}

impl From<serde_json::Error> for LspError {
    fn from(error: serde_json::Error) -> Self {
        Self::new(error.to_string())
    }
}

impl From<LspError> for super::CoreError {
    fn from(error: LspError) -> Self {
        match error.missing {
            Some(detail) => super::CoreError::of(ErrorKind::LanguageServerMissing, error.message)
                .with_detail(detail),
            None => error.message.into(),
        }
    }
}

fn lsp<E: std::fmt::Display>(e: E) -> LspError {
    LspError::new(e.to_string())
}

/// Each file's latest diagnostics, by path, one set per language server that
/// published any. A server keeps at most a bounded number of files.
fn diagnostics() -> &'static Mutex<HashMap<String, Vec<LspDiagnosticsPayload>>> {
    static DIAGNOSTICS: OnceLock<Mutex<HashMap<String, Vec<LspDiagnosticsPayload>>>> =
        OnceLock::new();
    DIAGNOSTICS.get_or_init(Default::default)
}

/// Keeps what a server published for a file and tells clients it moved.
fn publisher(core: Weak<Core>) -> Publish {
    Arc::new(move |payload: LspDiagnosticsPayload| {
        let (project, path) = (payload.project.clone(), payload.path.clone());
        {
            let mut kept = diagnostics().locked();
            let sets = kept.entry(path.clone()).or_default();
            sets.retain(|set| set.project != payload.project || set.language != payload.language);
            if !payload.diagnostics.is_empty() {
                sets.push(payload);
            }
            if sets.is_empty() {
                kept.remove(&path);
            }
        }
        if let Some(core) = core.upgrade() {
            core.changed(What::ReadIn(Topic::Lsp, Some(&project), Some(vec![path])));
        }
    })
}

fn close_all(keys: Vec<held::DocKey>) {
    if keys.is_empty() {
        return;
    }
    tokio::spawn(async move {
        for key in keys {
            let _ = lsp_close(key.project, key.language, key.path).await;
        }
    });
}

/// How long a client's documents wait for it to come back after its
/// connection goes, such as across a hand-over.
const CLAIM_GRACE: std::time::Duration = std::time::Duration::from_secs(30);

/// Closes every document `client` holds, as its page reloads.
pub(crate) fn release(client: ClientId) {
    close_all(held::leave(client));
}

/// The language servers, whose state is the process's own, as a part of the core.
pub(crate) struct Lsp;

impl super::part::Part for Lsp {
    /// Keeps `client`'s documents open a while after its connection goes, for it
    /// to claim back, then closes those it did not.
    fn client_gone(&self, _core: &Arc<Core>, client: ClientId) {
        let handles = held::orphan(client);
        if handles.is_empty() {
            return;
        }
        tokio::spawn(async move {
            tokio::time::sleep(CLAIM_GRACE).await;
            close_all(held::expire(&handles));
        });
    }
}

pub(crate) fn carry() -> Vec<held::Carried> {
    held::carry()
}

/// Opens again the documents a hand-over carried, starting their servers,
/// and gives their clients the grace to claim them back.
pub(crate) fn adopt(core: &Arc<Core>, carried: Vec<held::Carried>) {
    if carried.is_empty() {
        return;
    }
    held::adopt(&carried);
    let core = core.clone();
    tokio::spawn(async move {
        let mut started = std::collections::HashSet::new();
        for document in &carried {
            let server = (document.key.project.clone(), document.key.language.clone());
            if started.insert(server.clone()) {
                if let Err(error) = lsp_start(publisher(core.me.clone()), server.0, server.1).await
                {
                    eprintln!("sikemux core: a language server did not start again after the update: {error}");
                }
            }
            let key = document.key.clone();
            let _ = lsp_open(
                key.project,
                key.language,
                key.path,
                document.text.clone(),
                Some(document.language_id.clone()),
            )
            .await;
        }
        tokio::time::sleep(CLAIM_GRACE).await;
        let handles: Vec<String> = carried
            .into_iter()
            .map(|document| document.handle)
            .collect();
        close_all(held::expire(&handles));
    });
}

fn held_key(document: &LspDocument) -> held::DocKey {
    held::DocKey {
        project: document.project.clone(),
        language: document.language.clone(),
        path: document.path.clone(),
    }
}

pub(crate) async fn languages() -> CoreResult<Vec<String>> {
    Ok(lsp_languages().await?)
}

pub(crate) async fn install(params: LspLanguage) -> CoreResult<String> {
    Ok(lsp_install_server(params.language).await?)
}

pub(crate) async fn start(core: &Arc<Core>, params: LspServer) -> CoreResult<()> {
    Ok(lsp_start(publisher(core.me.clone()), params.project, params.language).await?)
}

pub(crate) async fn stop(params: LspProject) -> CoreResult<()> {
    Ok(lsp_stop(params.project).await?)
}

/// The handles of those of `params` that were still held, now `client`'s.
pub(crate) fn claim(client: ClientId, params: LspClaim) -> Vec<String> {
    held::claim(client, &params.handles)
}

pub(crate) async fn open(client: ClientId, params: LspOpen) -> CoreResult<LspOpened> {
    let LspOpen {
        document,
        content,
        language_id,
    } = params;
    let key = held_key(&document);
    let kept = content.clone();
    let language_id = language_id.unwrap_or_else(|| document.language.clone());
    lsp_open(
        document.project,
        document.language,
        document.path,
        content,
        Some(language_id.clone()),
    )
    .await?;
    let handle = held::hold(client, key, language_id, kept);
    Ok(LspOpened { handle })
}

pub(crate) async fn change(params: LspChange) -> CoreResult<()> {
    let LspChange {
        document,
        content,
        version,
    } = params;
    let key = held_key(&document);
    let kept = content.clone();
    lsp_change(
        document.project,
        document.language,
        document.path,
        content,
        version,
    )
    .await?;
    held::set_text(&key, kept);
    Ok(())
}

pub(crate) async fn change_incremental(params: LspIncrementalChange) -> CoreResult<()> {
    let LspIncrementalChange {
        document,
        changes,
        version,
    } = params;
    let key = held_key(&document);
    let applied = changes.clone();
    lsp_change_incremental(
        document.project,
        document.language,
        document.path,
        changes,
        version,
    )
    .await?;
    held::edit(&key, &applied);
    Ok(())
}

pub(crate) async fn save(params: LspSave) -> CoreResult<()> {
    let LspSave { document, content } = params;
    Ok(lsp_save(document.project, document.language, document.path, content).await?)
}

pub(crate) async fn close(client: ClientId, document: LspDocument) -> CoreResult<()> {
    held::let_go(client, &held_key(&document));
    Ok(lsp_close(document.project, document.language, document.path).await?)
}

pub(crate) async fn locations(params: LspLocations) -> CoreResult<Vec<LspLocation>> {
    let LspLocations {
        document,
        line,
        character,
        kind,
    } = params;
    Ok(lsp_locations(
        document.project,
        document.language,
        document.path,
        line,
        character,
        kind,
    )
    .await?)
}

pub(crate) async fn symbols(document: LspDocument) -> CoreResult<Vec<LspDocumentSymbol>> {
    Ok(lsp_document_symbols(document.project, document.language, document.path).await?)
}

/// What each language server last published for the file.
pub(crate) fn diagnostics_of(params: LspPath) -> Vec<LspDiagnosticsPayload> {
    (diagnostics().locked().get(&params.path).cloned()).unwrap_or_default()
}
