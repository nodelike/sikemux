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

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_process::sync::Locked;
use sikemux_wire::protocol::ErrorKind;

use commands::*;
pub(crate) use held::Carried;
pub(crate) use registry::drain_all;
use types::{LspDiagnosticsPayload, LspKind, LspTextChange};

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

#[derive(Deserialize)]
pub(crate) struct Server {
    project: String,
    language: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Document {
    project: String,
    language: String,
    path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Open {
    #[serde(flatten)]
    document: Document,
    content: String,
    language_id: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct Change {
    #[serde(flatten)]
    document: Document,
    content: String,
    version: u32,
}

#[derive(Deserialize)]
pub(crate) struct IncrementalChange {
    #[serde(flatten)]
    document: Document,
    changes: Vec<LspTextChange>,
    version: u32,
}

#[derive(Deserialize)]
pub(crate) struct Save {
    #[serde(flatten)]
    document: Document,
    content: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct Locations {
    #[serde(flatten)]
    document: Document,
    line: u32,
    character: u32,
    kind: LspKind,
}

#[derive(Deserialize)]
pub(crate) struct Language {
    language: String,
}

#[derive(Deserialize)]
pub(crate) struct Project {
    project: String,
}

#[derive(Deserialize)]
pub(crate) struct Path {
    path: String,
}

fn answer(value: impl Serialize) -> CoreResult<Value> {
    Ok(serde_json::to_value(value)?)
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

#[derive(Deserialize)]
pub(crate) struct Claim {
    handles: Vec<String>,
}

pub(crate) fn release_all(client: ClientId) -> CoreResult<Value> {
    release(client);
    answer(())
}

pub(crate) fn claim(client: ClientId, Claim { handles }: Claim) -> CoreResult<Value> {
    answer(held::claim(client, &handles))
}

pub(crate) async fn languages() -> CoreResult<Value> {
    answer(lsp_languages().await?)
}

pub(crate) async fn install(Language { language }: Language) -> CoreResult<Value> {
    answer(lsp_install_server(language).await?)
}

pub(crate) async fn start(
    core: &Arc<Core>,
    Server { project, language }: Server,
) -> CoreResult<Value> {
    answer(lsp_start(publisher(core.me.clone()), project, language).await?)
}

pub(crate) async fn stop(Project { project }: Project) -> CoreResult<Value> {
    answer(lsp_stop(project).await?)
}

pub(crate) async fn open(client: ClientId, params: Open) -> CoreResult<Value> {
    let Open {
        document,
        content,
        language_id,
    } = params;
    let key = held::DocKey {
        project: document.project.clone(),
        language: document.language.clone(),
        path: document.path.clone(),
    };
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
    answer(serde_json::json!({ "handle": handle }))
}

pub(crate) async fn change(params: Change) -> CoreResult<Value> {
    let Change {
        document,
        content,
        version,
    } = params;
    let key = held::DocKey {
        project: document.project.clone(),
        language: document.language.clone(),
        path: document.path.clone(),
    };
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
    answer(())
}

pub(crate) async fn change_incremental(params: IncrementalChange) -> CoreResult<Value> {
    let IncrementalChange {
        document,
        changes,
        version,
    } = params;
    let key = held::DocKey {
        project: document.project.clone(),
        language: document.language.clone(),
        path: document.path.clone(),
    };
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
    answer(())
}

pub(crate) async fn save(Save { document, content }: Save) -> CoreResult<Value> {
    answer(lsp_save(document.project, document.language, document.path, content).await?)
}

pub(crate) async fn close(client: ClientId, document: Document) -> CoreResult<Value> {
    let Document {
        project,
        language,
        path,
    } = document;
    held::let_go(
        client,
        &held::DocKey {
            project: project.clone(),
            language: language.clone(),
            path: path.clone(),
        },
    );
    answer(lsp_close(project, language, path).await?)
}

pub(crate) async fn locations(params: Locations) -> CoreResult<Value> {
    let Locations {
        document,
        line,
        character,
        kind,
    } = params;
    answer(
        lsp_locations(
            document.project,
            document.language,
            document.path,
            line,
            character,
            kind,
        )
        .await?,
    )
}

pub(crate) async fn symbols(document: Document) -> CoreResult<Value> {
    let Document {
        project,
        language,
        path,
    } = document;
    answer(lsp_document_symbols(project, language, path).await?)
}

pub(crate) fn diagnostics_of(Path { path }: Path) -> CoreResult<Value> {
    answer(
        diagnostics()
            .locked()
            .get(&path)
            .cloned()
            .unwrap_or_default(),
    )
}
