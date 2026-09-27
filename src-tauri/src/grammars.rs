//! Syntax grammars the app does not ship are fetched the first time a file
//! needs one, checked against the hash recorded when the app was built, and
//! kept on disk so each is downloaded once.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use futures::StreamExt;
use reqwest::Client;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager};

use crate::error::{AppError, AppResult};

const MIRRORS: [&str; 2] = [
    "https://cdn.jsdelivr.net/npm/@shikijs/langs@",
    "https://unpkg.com/@shikijs/langs@",
];
const FETCH_TIMEOUT: Duration = Duration::from_secs(20);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// The largest grammar is under 800 KB.
const MAX_BYTES: usize = 2 * 1024 * 1024;
/// A file shown while offline asks again on every render; after a failed
/// download the network is left alone for this long.
const RETRY_AFTER: Duration = Duration::from_secs(60);
const GRAMMAR_PREFIX: &str = "const lang = Object.freeze(JSON.parse(";
const GRAMMAR_SUFFIX: &str = "))";

#[derive(Deserialize)]
struct Manifest {
    version: String,
    sha256: HashMap<String, String>,
}

fn manifest() -> &'static Manifest {
    static MANIFEST: OnceLock<Manifest> = OnceLock::new();
    MANIFEST.get_or_init(|| {
        serde_json::from_str(include_str!("../grammars.json"))
            .expect("grammars.json is generated and always parses")
    })
}

/// The grammar named `id`, as the JSON text Shiki loads.
#[tauri::command]
pub async fn grammar_load(app: AppHandle, id: String) -> AppResult<String> {
    let manifest = manifest();
    let expected = manifest
        .sha256
        .get(&id)
        .ok_or(AppError::BadArg("unknown grammar"))?;
    let path = app
        .path()
        .app_cache_dir()
        .map_err(|err| AppError::Fs(err.to_string()))?
        .join("grammars")
        .join(&manifest.version)
        .join(format!("{id}.mjs"));

    if let Ok(source) = std::fs::read(&path) {
        if digest(&source) == *expected {
            return grammar_text(&source);
        }
    }
    if recently_failed(&id) {
        return Err(AppError::Http(format!("grammar {id} is unavailable")));
    }
    match download(&manifest.version, &id, expected).await {
        Ok(source) => {
            keep(&path, &source)?;
            grammar_text(&source)
        }
        Err(err) => {
            remember_failure(&id);
            Err(err)
        }
    }
}

async fn download(version: &str, id: &str, expected: &str) -> AppResult<Vec<u8>> {
    let mut last = AppError::Http(format!("grammar {id} could not be fetched"));
    for mirror in MIRRORS {
        match fetch(&format!("{mirror}{version}/dist/{id}.mjs")).await {
            Ok(source) if digest(&source) == expected => return Ok(source),
            Ok(_) => last = AppError::Http(format!("grammar {id} did not match its hash")),
            Err(err) => last = err,
        }
    }
    Err(last)
}

async fn fetch(url: &str) -> AppResult<Vec<u8>> {
    let response = client().get(url).send().await?.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_BYTES as u64)
    {
        return Err(AppError::Http(format!("{url} is too large")));
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if body.len() + chunk.len() > MAX_BYTES {
            return Err(AppError::Http(format!("{url} is too large")));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Written beside its final name and renamed, so a reader never sees half a file.
fn keep(path: &Path, source: &[u8]) -> AppResult<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let partial = PathBuf::from(format!("{}.{}.partial", path.display(), std::process::id()));
    std::fs::write(&partial, source)?;
    std::fs::rename(&partial, path)?;
    Ok(())
}

/// Each module holds its grammar as one JSON string literal, so reading that
/// literal as JSON gives back the grammar's own text without running the module.
fn grammar_text(source: &[u8]) -> AppResult<String> {
    let source = std::str::from_utf8(source).map_err(|err| AppError::Other(err.to_string()))?;
    let literal = source
        .lines()
        .find_map(|line| {
            line.strip_prefix(GRAMMAR_PREFIX)?
                .strip_suffix(GRAMMAR_SUFFIX)
        })
        .ok_or_else(|| AppError::Other("grammar module has no grammar".into()))?;
    Ok(serde_json::from_str(literal)?)
}

fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn failures() -> &'static Mutex<HashMap<String, Instant>> {
    static FAILURES: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    FAILURES.get_or_init(Default::default)
}

fn recently_failed(id: &str) -> bool {
    failures()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(id)
        .is_some_and(|at| at.elapsed() < RETRY_AFTER)
}

fn remember_failure(id: &str) {
    failures()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(id.to_owned(), Instant::now());
}

fn client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        Client::builder()
            .timeout(FETCH_TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .user_agent(concat!("sikemux/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_grammar_has_a_sha256() {
        let manifest = manifest();
        assert!(manifest.sha256.contains_key("astro"));
        assert!(manifest
            .sha256
            .values()
            .all(|hash| hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())));
    }

    #[test]
    fn the_grammar_is_read_out_of_its_module_without_running_it() {
        let source = concat!(
            "import json from './json.mjs'\n\n",
            r#"const lang = Object.freeze(JSON.parse("{\"name\":\"astro\",\"patterns\":[]}"))"#,
            "\n\nexport default [\n...json,\nlang\n]\n",
        );
        assert_eq!(
            grammar_text(source.as_bytes()).unwrap(),
            r#"{"name":"astro","patterns":[]}"#
        );
    }

    #[test]
    fn a_module_without_a_grammar_is_refused() {
        assert!(grammar_text(b"export { default } from './shellscript.mjs'").is_err());
    }
}
