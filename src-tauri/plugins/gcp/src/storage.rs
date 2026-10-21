// Cloud Storage: buckets, browsing their objects by folder, a peek at an
// object's start, and downloading one to the Downloads folder.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::Target;
use crate::client::{self, at, encoded, segment, text};
use crate::error::{GcpError, GcpResult};

const API: &str = "https://storage.googleapis.com/storage/v1";
const MAX_BUCKETS: usize = 1000;
const MAX_OBJECTS: usize = 1000;
const PREVIEW_BYTES: u64 = 64 * 1024;
const MAX_DOWNLOAD_BYTES: u64 = 512 * 1024 * 1024;

pub fn bucket(value: &Value) -> Value {
    json!({
        "name": at(value, "/name"),
        "location": at(value, "/location"),
        "locationType": at(value, "/locationType"),
        "storageClass": at(value, "/storageClass"),
        "publicAccessPrevention": at(value, "/iamConfiguration/publicAccessPrevention"),
        "uniformAccess": at(value, "/iamConfiguration/uniformBucketLevelAccess/enabled").as_bool() == Some(true),
        "versioning": at(value, "/versioning/enabled").as_bool() == Some(true),
        "labels": at(value, "/labels"),
        "createdAt": at(value, "/timeCreated"),
        "updatedAt": at(value, "/updated"),
    })
}

pub fn object(value: &Value) -> Value {
    json!({
        "name": at(value, "/name"),
        "size": at(value, "/size").as_str().and_then(|size| size.parse::<u64>().ok()),
        "contentType": at(value, "/contentType"),
        "storageClass": at(value, "/storageClass"),
        "updatedAt": at(value, "/updated"),
        "generation": at(value, "/generation"),
    })
}

#[derive(Deserialize)]
pub struct Listing {
    #[serde(flatten)]
    pub target: Target,
    #[serde(default)]
    pub query: Option<String>,
}

pub async fn buckets(input: Listing) -> GcpResult<Vec<Value>> {
    let project = input.target.project()?;
    let listed = client::list(
        &input.target,
        &format!("{API}/b"),
        &[("project", project), ("maxResults", "500".into())],
        "items",
        MAX_BUCKETS,
    )
    .await?;
    let words = input.query.unwrap_or_default().trim().to_lowercase();
    Ok(listed
        .iter()
        .map(bucket)
        .filter(|each| words.is_empty() || text(each, "/name").contains(&words))
        .collect())
}

#[derive(Deserialize)]
pub struct Folder {
    #[serde(flatten)]
    pub target: Target,
    pub bucket: String,
    /// The folder to list, ending in a slash; none lists the bucket's top.
    #[serde(default)]
    pub prefix: Option<String>,
}

/// One folder of a bucket: the folders inside it and its objects.
pub async fn objects(input: Folder) -> GcpResult<Value> {
    let bucket = segment("bucket", &input.bucket)?;
    let prefix = input.prefix.unwrap_or_default();
    let url = format!("{API}/b/{bucket}/o");
    let mut folders = Vec::new();
    let mut objects = Vec::new();
    let mut page_token: Option<String> = None;
    loop {
        let mut query = vec![
            ("delimiter", "/".to_string()),
            ("prefix", prefix.clone()),
            ("maxResults", "500".to_string()),
        ];
        if let Some(token) = &page_token {
            query.push(("pageToken", token.clone()));
        }
        let page = client::get(&input.target, &url, &query).await?;
        folders.extend(
            at(&page, "/prefixes")
                .as_array()
                .cloned()
                .unwrap_or_default(),
        );
        objects.extend(
            at(&page, "/items")
                .as_array()
                .into_iter()
                .flatten()
                .filter(|item| text(item, "/name") != prefix)
                .map(object),
        );
        page_token = page
            .get("nextPageToken")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
            .map(str::to_string);
        if page_token.is_none() || objects.len() + folders.len() >= MAX_OBJECTS {
            break;
        }
    }
    Ok(
        json!({ "bucket": bucket, "prefix": prefix, "folders": folders, "objects": objects, "more": page_token.is_some() }),
    )
}

#[derive(Deserialize)]
pub struct ObjectRef {
    #[serde(flatten)]
    pub target: Target,
    pub bucket: String,
    pub object: String,
}

fn object_url(input: &ObjectRef) -> GcpResult<String> {
    let bucket = segment("bucket", &input.bucket)?;
    if input.object.is_empty() {
        return Err(GcpError::BadArg("no object was named".into()));
    }
    Ok(format!("{API}/b/{bucket}/o/{}", encoded(&input.object)))
}

fn looks_like_text(content_type: &str, bytes: &[u8]) -> bool {
    content_type.starts_with("text/")
        || ["json", "xml", "yaml", "javascript", "csv", "x-sh", "toml"]
            .iter()
            .any(|kind| content_type.contains(kind))
        || (!bytes.is_empty()
            && std::str::from_utf8(bytes.get(..bytes.len().min(4096)).unwrap_or_default()).is_ok()
            && !bytes.contains(&0))
}

/// An object's details and, for text, its first 64 KiB.
pub async fn preview(input: ObjectRef) -> GcpResult<Value> {
    let url = object_url(&input)?;
    let raw = client::get(&input.target, &url, &[]).await?;
    let mut read = object(&raw);
    let content_type = text(&raw, "/contentType");
    let bytes = client::bytes(
        &input.target,
        &url,
        &[("alt", "media".into())],
        Some(PREVIEW_BYTES),
    )
    .await?;
    let shown = if looks_like_text(&content_type, &bytes) {
        json!(String::from_utf8_lossy(&bytes))
    } else {
        Value::Null
    };
    if let Some(object) = read.as_object_mut() {
        object.insert("bucket".into(), json!(input.bucket));
        object.insert("preview".into(), shown);
        object.insert(
            "trimmed".into(),
            json!(at(&raw, "/size")
                .as_str()
                .and_then(|size| size.parse::<u64>().ok())
                .is_some_and(|size| size > PREVIEW_BYTES)),
        );
        object.insert(
            "uri".into(),
            json!(format!("gs://{}/{}", input.bucket, input.object)),
        );
    }
    Ok(read)
}

/// A free path in `dir` for `name`: the name itself, or `name (2).ext` and so on.
pub fn free_path(dir: &Path, name: &str) -> PathBuf {
    let name = Path::new(name)
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty() && *n != "." && *n != "..")
        .unwrap_or("download");
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem.to_string(), format!(".{extension}")),
        _ => (name.to_string(), String::new()),
    };
    (2..10_000)
        .map(|n| dir.join(format!("{stem} ({n}){extension}")))
        .find(|path| !path.exists())
        .unwrap_or(candidate)
}

/// Saves an object in the Downloads folder, answering with where it went.
pub async fn download(input: ObjectRef) -> GcpResult<Value> {
    let url = object_url(&input)?;
    let raw = client::get(&input.target, &url, &[]).await?;
    let size = at(&raw, "/size")
        .as_str()
        .and_then(|size| size.parse::<u64>().ok())
        .unwrap_or(0);
    if size > MAX_DOWNLOAD_BYTES {
        return Err(GcpError::BadArg(format!(
            "the object is {} MiB; use gsutil or gcloud storage cp for one this large",
            size / (1024 * 1024)
        )));
    }
    let bytes = client::bytes(&input.target, &url, &[("alt", "media".into())], None).await?;
    let home =
        std::env::var_os("HOME").ok_or_else(|| GcpError::Transport("no home folder".into()))?;
    let downloads = PathBuf::from(home).join("Downloads");
    let path = free_path(&downloads, &input.object);
    tokio::task::spawn_blocking({
        let path = path.clone();
        move || {
            std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))
                .and_then(|_| std::fs::write(&path, bytes))
        }
    })
    .await
    .map_err(|error| GcpError::Transport(error.to_string()))?
    .map_err(|error| GcpError::Transport(format!("saving the object: {error}")))?;
    Ok(json!({ "path": path.to_string_lossy(), "size": size }))
}

/// The content type a file's extension suggests, so a browser shows it rather than downloading it.
pub fn content_type(name: &str) -> &'static str {
    let extension = name
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" | "mjs" => "text/javascript",
        "json" => "application/json",
        "txt" | "log" | "md" => "text/plain",
        "csv" => "text/csv",
        "xml" => "application/xml",
        "yaml" | "yml" => "application/yaml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "gz" | "tgz" => "application/gzip",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

#[derive(Deserialize)]
pub struct Upload {
    #[serde(flatten)]
    pub target: Target,
    pub bucket: String,
    /// The folder it goes in, ending in a slash; none puts it at the bucket's top.
    #[serde(default)]
    pub prefix: Option<String>,
    /// The file on this Mac to upload.
    pub path: PathBuf,
}

/// Stores a file from this Mac in a bucket's folder, under the file's own name.
pub async fn upload(input: Upload) -> GcpResult<Value> {
    let bucket = segment("bucket", &input.bucket)?;
    let name = input
        .path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| GcpError::BadArg("no file was named".into()))?
        .to_string();
    let path = input.path.clone();
    let bytes = tokio::task::spawn_blocking(move || -> GcpResult<Vec<u8>> {
        let size = std::fs::metadata(&path)
            .map_err(|error| GcpError::BadArg(format!("reading the file: {error}")))?
            .len();
        if size > MAX_DOWNLOAD_BYTES {
            return Err(GcpError::BadArg(format!(
                "the file is {} MiB; use gcloud storage cp for one this large",
                size / (1024 * 1024)
            )));
        }
        std::fs::read(&path).map_err(|error| GcpError::BadArg(format!("reading the file: {error}")))
    })
    .await
    .map_err(|error| GcpError::Transport(error.to_string()))??;
    let key = format!("{}{name}", input.prefix.unwrap_or_default());
    let stored = client::upload(
        &input.target,
        &format!("https://storage.googleapis.com/upload/storage/v1/b/{bucket}/o"),
        &[("uploadType", "media".into()), ("name", key)],
        bytes,
        content_type(&name),
    )
    .await?;
    Ok(object(&stored))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets_and_objects_read_plainly() {
        let read = bucket(
            &json!({ "name": "acme-assets", "location": "US", "storageClass": "STANDARD", "iamConfiguration": { "publicAccessPrevention": "enforced", "uniformBucketLevelAccess": { "enabled": true } }, "versioning": { "enabled": true } }),
        );
        assert_eq!(read["uniformAccess"], true);
        assert_eq!(read["publicAccessPrevention"], "enforced");
        let file =
            object(&json!({ "name": "logs/app.log", "size": "2048", "contentType": "text/plain" }));
        assert_eq!(file["size"], 2048);
    }

    #[test]
    fn text_is_told_apart_from_binary() {
        assert!(looks_like_text("application/json", b"{}"));
        assert!(looks_like_text("application/octet-stream", b"plain words"));
        assert!(!looks_like_text(
            "image/png",
            &[0x89, b'P', b'N', b'G', 0, 0]
        ));
    }

    #[test]
    fn a_files_extension_names_its_type() {
        assert_eq!(content_type("index.HTML"), "text/html");
        assert_eq!(content_type("report.csv"), "text/csv");
        assert_eq!(content_type("photo.jpeg"), "image/jpeg");
        assert_eq!(content_type("backup"), "application/octet-stream");
        assert_eq!(content_type("archive.tar.gz"), "application/gzip");
    }

    #[test]
    fn a_download_never_replaces_a_file() {
        let dir =
            std::env::temp_dir().join(format!("sikemux-gcp-downloads-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a folder");
        assert_eq!(free_path(&dir, "logs/app.log"), dir.join("app.log"));
        std::fs::write(dir.join("app.log"), b"x").expect("a file");
        assert_eq!(free_path(&dir, "logs/app.log"), dir.join("app (2).log"));
        assert_eq!(free_path(&dir, "../"), dir.join("download"));
        std::fs::remove_dir_all(dir).ok();
    }
}
