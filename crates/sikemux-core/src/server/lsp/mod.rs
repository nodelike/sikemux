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

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sikemux_process::sync::Locked;

use commands::*;
pub(crate) use held::Carried;
pub(crate) use registry::drain_all;
use types::{LspDiagnosticsPayload, LspKind, LspTextChange};

use super::changed::Topic;
use super::connection::ClientId;
use super::{Core, CoreError, CoreResult};

/// Takes each file's diagnostics as a server publishes them.
pub(crate) type Publish = Arc<dyn Fn(LspDiagnosticsPayload) + Send + Sync>;

/// What went wrong, under the category clients branch on: `lsp`, or
/// `lsp-server-missing` when the server's program is not installed.
#[derive(Debug)]
pub(crate) struct LspError(String);

pub(crate) type LspResult<T> = Result<T, LspError>;

impl LspError {
    pub(crate) fn new(message: String) -> Self {
        Self(format!("lsp: {message}"))
    }

    pub(crate) fn missing(language: &str, bin: &str) -> Self {
        Self(format!(
            "lsp-server-missing: lsp server `{bin}` for {language} not found"
        ))
    }
}

impl std::fmt::Display for LspError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
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
        super::CoreError::from(error.0)
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
            core.changes
                .changed(Topic::Lsp, Some(&project), Some(vec![path]));
        }
    })
}

#[derive(Deserialize)]
struct Server {
    project: String,
    language: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Document {
    project: String,
    language: String,
    path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Open {
    #[serde(flatten)]
    document: Document,
    content: String,
    language_id: Option<String>,
}

#[derive(Deserialize)]
struct Change {
    #[serde(flatten)]
    document: Document,
    content: String,
    version: u32,
}

#[derive(Deserialize)]
struct IncrementalChange {
    #[serde(flatten)]
    document: Document,
    changes: Vec<LspTextChange>,
    version: u32,
}

#[derive(Deserialize)]
struct Save {
    #[serde(flatten)]
    document: Document,
    content: Option<String>,
}

#[derive(Deserialize)]
struct Locations {
    #[serde(flatten)]
    document: Document,
    line: u32,
    character: u32,
    kind: LspKind,
}

#[derive(Deserialize)]
struct Language {
    language: String,
}

#[derive(Deserialize)]
struct Project {
    project: String,
}

#[derive(Deserialize)]
struct Path {
    path: String,
}

fn parse<T: for<'de> Deserialize<'de>>(params: Value) -> CoreResult<T> {
    serde_json::from_value(params).map_err(|error| {
        CoreError::from(format!(
            "invalid argument: the call's params do not fit: {error}"
        ))
    })
}

fn answer(value: impl Serialize) -> CoreResult<Value> {
    Ok(serde_json::to_value(value)?)
}

/// Closes documents nobody holds any more.
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

/// Keeps `client`'s documents open a while after its connection goes, for it
/// to claim back, then closes those it did not.
pub(crate) fn orphan(client: ClientId) {
    let handles = held::orphan(client);
    if handles.is_empty() {
        return;
    }
    tokio::spawn(async move {
        tokio::time::sleep(CLAIM_GRACE).await;
        close_all(held::expire(&handles));
    });
}

/// The documents to carry through a hand-over.
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
struct Claim {
    handles: Vec<String>,
}

/// The editor's language-server methods, `lsp.<name>`, as `client` asks.
pub(crate) async fn call(
    core: &Arc<Core>,
    client: ClientId,
    method: &str,
    params: Value,
) -> CoreResult<Value> {
    match method {
        "lsp.release" => {
            release(client);
            answer(())
        }
        "lsp.claim" => {
            let Claim { handles } = parse(params)?;
            answer(held::claim(client, &handles))
        }
        "lsp.languages" => answer(lsp_languages().await?),
        "lsp.install" => answer(lsp_install_server(parse::<Language>(params)?.language).await?),
        "lsp.start" => {
            let Server { project, language } = parse(params)?;
            answer(lsp_start(publisher(core.me.clone()), project, language).await?)
        }
        "lsp.stop" => answer(lsp_stop(parse::<Project>(params)?.project).await?),
        "lsp.open" => {
            let Open {
                document,
                content,
                language_id,
            } = parse(params)?;
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
        "lsp.change" => {
            let Change {
                document,
                content,
                version,
            } = parse(params)?;
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
        "lsp.changeIncremental" => {
            let IncrementalChange {
                document,
                changes,
                version,
            } = parse(params)?;
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
        "lsp.save" => {
            let Save { document, content } = parse(params)?;
            answer(lsp_save(document.project, document.language, document.path, content).await?)
        }
        "lsp.close" => {
            let Document {
                project,
                language,
                path,
            } = parse(params)?;
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
        "lsp.locations" => {
            let Locations {
                document,
                line,
                character,
                kind,
            } = parse(params)?;
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
        "lsp.symbols" => {
            let Document {
                project,
                language,
                path,
            } = parse(params)?;
            answer(lsp_document_symbols(project, language, path).await?)
        }
        "lsp.diagnostics" => {
            let Path { path } = parse(params)?;
            answer(
                diagnostics()
                    .locked()
                    .get(&path)
                    .cloned()
                    .unwrap_or_default(),
            )
        }
        _ => Err(format!("this host does not know {method}").into()),
    }
}
