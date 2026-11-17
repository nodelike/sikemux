// The Play Developer API: every change is made inside an edit, which nothing sees
// until it is committed and which is dropped when it is not.

use std::path::Path;

use futures::stream;
use reqwest::{Body, Method};
use serde_json::Value;
use sikemux_plugin_api::http::{Http, Read, Redirects};
use sikemux_plugin_api::json::text;
use tokio::io::AsyncReadExt;

use crate::auth;
use crate::error::{GooglePlay, PlayResult};

const API: &str = "https://androidpublisher.googleapis.com/androidpublisher/v3/applications";
const UPLOAD: &str =
    "https://androidpublisher.googleapis.com/upload/androidpublisher/v3/applications";
const CHUNK: usize = 1024 * 1024;

pub static HTTP: Http<GooglePlay> = Http::new(Redirects::Follow, 4);

pub struct Edit {
    token: String,
    package: String,
    pub id: String,
}

impl Edit {
    pub async fn open(data_dir: &Path, package: &str) -> PlayResult<Edit> {
        let token = auth::token(data_dir).await?;
        let package = package.trim().to_string();
        let opened = request(
            &token,
            Method::POST,
            &format!("{API}/{package}/edits"),
            None,
        )
        .await?;
        let id = text(&opened, "/id");
        Ok(Edit { token, package, id })
    }

    fn at(&self, path: &str) -> String {
        format!("{API}/{}/edits/{}{path}", self.package, self.id)
    }

    pub async fn get(&self, path: &str) -> PlayResult<Value> {
        request(&self.token, Method::GET, &self.at(path), None).await
    }

    pub async fn put(&self, path: &str, body: Value) -> PlayResult<Value> {
        request(&self.token, Method::PUT, &self.at(path), Some(body)).await
    }

    pub async fn commit(&self) -> PlayResult<Value> {
        request(&self.token, Method::POST, &self.at(":commit"), None).await
    }

    pub async fn discard(self) {
        let _ = request(&self.token, Method::DELETE, &self.at(""), None).await;
    }

    /// Sends the bundle as it is read, so a large one never sits in memory whole.
    pub async fn upload_bundle(&self, file: &Path) -> PlayResult<Value> {
        let opened = tokio::fs::File::open(file).await?;
        let size = opened.metadata().await?.len();
        let chunks = stream::unfold(opened, |mut file| async move {
            let mut chunk = vec![0; CHUNK];
            match file.read(&mut chunk).await {
                Ok(0) => None,
                Ok(read) => {
                    chunk.truncate(read);
                    Some((Ok(chunk), file))
                }
                Err(error) => Some((Err(error), file)),
            }
        });
        let request = HTTP
            .transfer_client()?
            .post(format!(
                "{UPLOAD}/{}/edits/{}/bundles?uploadType=media",
                self.package, self.id
            ))
            .bearer_auth(&self.token)
            .header("content-type", "application/octet-stream")
            .header("content-length", size)
            .body(Body::wrap_stream(chunks));
        HTTP.ask(request, Read::ALL).await?.succeeded()?.parsed()
    }
}

async fn request(token: &str, method: Method, url: &str, body: Option<Value>) -> PlayResult<Value> {
    let mut builder = HTTP.client()?.request(method, url).bearer_auth(token);
    builder = match body {
        Some(body) => builder.json(&body),
        None => builder.header("content-length", 0),
    };
    let answer = HTTP.ask(builder, Read::ALL).await?.succeeded()?;
    match answer.bytes.is_empty() {
        true => Ok(Value::Null),
        false => answer.parsed(),
    }
}
