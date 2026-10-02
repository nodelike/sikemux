//! Shows a local file or folder in a tab without letting tabs load `file://`.
//! Each folder gets its own loopback server that answers only under a random
//! first path segment, so other pages and processes cannot guess its address.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use url::Url;

use crate::file_serving::{content_type, parse_range, percent_decode};

const MAX_SHARES: usize = 16;
const MAX_REQUEST_HEAD: usize = 16 * 1024;

#[derive(Default)]
pub struct LocalFiles {
    shares: Mutex<HashMap<PathBuf, Share>>,
}

#[derive(Clone)]
struct Share {
    port: u16,
    token: String,
}

/// The local path a navigate target names: a `file://` URL, an absolute path or a `~/` path.
pub fn local_target(target: &str) -> Option<PathBuf> {
    let target = target.trim();
    if target.len() >= 5 && target[..5].eq_ignore_ascii_case("file:") {
        return Url::parse(target).ok()?.to_file_path().ok();
    }
    if let Some(rest) = target.strip_prefix("~/") {
        return Some(PathBuf::from(std::env::var("HOME").ok()?).join(rest));
    }
    target.starts_with('/').then(|| PathBuf::from(target))
}

impl LocalFiles {
    /// A loopback address that shows `path`, serving the folder it is in.
    pub fn url_for(&self, path: &Path) -> Result<String, String> {
        let path = path
            .canonicalize()
            .map_err(|_| format!("{} does not exist", path.display()))?;
        let (root, file) = if path.is_dir() {
            (path, None)
        } else {
            let root = path
                .parent()
                .ok_or("that file has no folder")?
                .to_path_buf();
            (
                root,
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned()),
            )
        };
        if let Some(name) = &file {
            if name.starts_with('.') {
                return Err("hidden files are not shown in a tab".into());
            }
        }
        refuse_broad_root(&root)?;
        let share = self.share(&root)?;
        let mut url =
            Url::parse(&format!("http://127.0.0.1:{}/", share.port)).expect("loopback url");
        {
            let mut segments = url.path_segments_mut().expect("http url has a path");
            segments.pop_if_empty().push(&share.token);
            segments.push(file.as_deref().unwrap_or(""));
        }
        Ok(url.into())
    }

    fn share(&self, root: &Path) -> Result<Share, String> {
        let mut shares = self
            .shares
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if let Some(share) = shares.get(root) {
            return Ok(share.clone());
        }
        if shares.len() >= MAX_SHARES {
            return Err(format!(
                "already showing {MAX_SHARES} local folders; restart Sikemux to show another"
            ));
        }
        let listener =
            std::net::TcpListener::bind(("127.0.0.1", 0)).map_err(|error| error.to_string())?;
        listener
            .set_nonblocking(true)
            .map_err(|error| error.to_string())?;
        let share = Share {
            port: listener
                .local_addr()
                .map_err(|error| error.to_string())?
                .port(),
            token: uuid::Uuid::new_v4().simple().to_string(),
        };
        let folder = Folder {
            root: root.to_path_buf(),
            token: share.token.clone(),
            port: share.port,
        };
        tauri::async_runtime::spawn(async move {
            let Ok(listener) = TcpListener::from_std(listener) else {
                return;
            };
            folder.serve(listener).await;
        });
        shares.insert(root.to_path_buf(), share.clone());
        Ok(share)
    }
}

/// A page can read every file under the folder it came from, so the whole disk or home folder is off limits.
fn refuse_broad_root(root: &Path) -> Result<(), String> {
    let home = std::env::var("HOME")
        .ok()
        .and_then(|home| Path::new(&home).canonicalize().ok());
    let too_broad = root.parent().is_none() || home.is_some_and(|home| home.starts_with(root));
    if too_broad {
        return Err(format!(
            "{} is too broad to show; open a file or folder inside a project",
            root.display()
        ));
    }
    Ok(())
}

#[derive(Clone)]
struct Folder {
    root: PathBuf,
    token: String,
    port: u16,
}

#[derive(Debug, PartialEq)]
enum Answer {
    File(PathBuf),
    Page(String),
    Redirect(String),
    Refused(u16),
}

impl Folder {
    async fn serve(self, listener: TcpListener) {
        while let Ok((stream, _)) = listener.accept().await {
            let folder = self.clone();
            tauri::async_runtime::spawn(async move {
                let _ = folder.handle(stream).await;
            });
        }
    }

    async fn handle(&self, mut stream: TcpStream) -> std::io::Result<()> {
        let Some(head) = read_head(&mut stream).await? else {
            return Ok(());
        };
        let request = Request::parse(&head);
        let head_only = request.method == "HEAD";
        let answer = self.answer(&request);
        match answer {
            Answer::File(path) => send_file(&mut stream, &path, request.range, head_only).await,
            Answer::Page(html) => {
                let head = response_head(
                    200,
                    "OK",
                    "text/html; charset=utf-8",
                    html.len() as u64,
                    &[],
                );
                stream.write_all(head.as_bytes()).await?;
                if !head_only {
                    stream.write_all(html.as_bytes()).await?;
                }
                Ok(())
            }
            Answer::Redirect(location) => {
                let head = response_head(
                    301,
                    "Moved Permanently",
                    "text/plain",
                    0,
                    &[("Location", &location)],
                );
                stream.write_all(head.as_bytes()).await
            }
            Answer::Refused(status) => {
                let reason = match status {
                    403 => "Forbidden",
                    405 => "Method Not Allowed",
                    _ => "Not Found",
                };
                let head = response_head(status, reason, "text/plain", reason.len() as u64, &[]);
                stream.write_all(head.as_bytes()).await?;
                stream.write_all(reason.as_bytes()).await
            }
        }
    }

    fn answer(&self, request: &Request) -> Answer {
        if !matches!(request.method.as_str(), "GET" | "HEAD") {
            return Answer::Refused(405);
        }
        let allowed_hosts = [
            format!("127.0.0.1:{}", self.port),
            format!("localhost:{}", self.port),
        ];
        if !request
            .host
            .as_deref()
            .is_some_and(|host| allowed_hosts.iter().any(|allowed| allowed == host))
        {
            return Answer::Refused(403);
        }
        let path = request.target.split(['?', '#']).next().unwrap_or_default();
        let prefix = format!("/{}", self.token);
        let Some(rest) = path.strip_prefix(&prefix) else {
            return Answer::Refused(404);
        };
        if rest.is_empty() {
            return Answer::Redirect(format!("{prefix}/"));
        }
        let Some(rest) = rest.strip_prefix('/') else {
            return Answer::Refused(404);
        };
        let Some(relative) = safe_relative(rest) else {
            return Answer::Refused(404);
        };
        let Ok(found) = self.root.join(&relative).canonicalize() else {
            return Answer::Refused(404);
        };
        if !found.starts_with(&self.root) {
            return Answer::Refused(404);
        }
        if found.is_file() {
            return Answer::File(found);
        }
        if !path.ends_with('/') {
            return Answer::Redirect(format!("{path}/"));
        }
        let index = found.join("index.html");
        if index.is_file() {
            return Answer::File(index);
        }
        Answer::Page(listing(&found, &relative))
    }
}

/// The request path as folder names under the root, refusing `..`, hidden names and anything undecodable.
fn safe_relative(rest: &str) -> Option<PathBuf> {
    let mut relative = PathBuf::new();
    for segment in rest.split('/').filter(|segment| !segment.is_empty()) {
        let name = percent_decode(segment)?;
        if name.starts_with('.') || name.contains(['/', '\\', '\0']) {
            return None;
        }
        relative.push(name);
    }
    Some(relative)
}

fn listing(folder: &Path, relative: &Path) -> String {
    let mut names: Vec<(String, bool)> = std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            (!name.starts_with('.')).then(|| (name, entry.path().is_dir()))
        })
        .collect();
    names.sort();
    let title = escape_html(&format!("/{}", relative.display()));
    let items: String = names
        .iter()
        .map(|(name, is_dir)| {
            let slash = if *is_dir { "/" } else { "" };
            let href = encode_segment(name);
            format!(
                "<li><a href=\"{href}{slash}\">{}{slash}</a></li>\n",
                escape_html(name)
            )
        })
        .collect();
    format!("<!doctype html>\n<meta charset=\"utf-8\">\n<title>{title}</title>\n<h1>{title}</h1>\n<ul>\n{items}</ul>\n")
}

fn encode_segment(name: &str) -> String {
    name.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

struct Request {
    method: String,
    target: String,
    host: Option<String>,
    range: Option<(u64, Option<u64>)>,
}

impl Request {
    fn parse(head: &str) -> Self {
        let mut lines = head.split("\r\n");
        let mut first = lines.next().unwrap_or_default().split(' ');
        let method = first.next().unwrap_or_default().to_owned();
        let target = first.next().unwrap_or_default().to_owned();
        let mut host = None;
        let mut range = None;
        for line in lines {
            let Some((name, value)) = line.split_once(':') else {
                continue;
            };
            let value = value.trim();
            if name.eq_ignore_ascii_case("host") {
                host = Some(value.to_ascii_lowercase());
            } else if name.eq_ignore_ascii_case("range") {
                range = parse_range(value);
            }
        }
        Self {
            method,
            target,
            host,
            range,
        }
    }
}

async fn read_head(stream: &mut TcpStream) -> std::io::Result<Option<String>> {
    let mut head = Vec::new();
    let mut buffer = [0u8; 2048];
    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
        if head.len() > MAX_REQUEST_HEAD {
            return Ok(None);
        }
        let read = stream.read(&mut buffer).await?;
        if read == 0 {
            return Ok(None);
        }
        head.extend_from_slice(&buffer[..read]);
    }
    Ok(Some(String::from_utf8_lossy(&head).into_owned()))
}

async fn send_file(
    stream: &mut TcpStream,
    path: &Path,
    range: Option<(u64, Option<u64>)>,
    head_only: bool,
) -> std::io::Result<()> {
    let mut file = std::fs::File::open(path)?;
    let size = file.metadata()?.len();
    let kind = content_type(path);
    let (start, end) = match range {
        Some((start, end)) if start < size => (start, end.unwrap_or(size - 1).min(size - 1)),
        Some(_) => {
            let range = format!("bytes */{size}");
            let head = response_head(
                416,
                "Range Not Satisfiable",
                kind,
                0,
                &[("Content-Range", &range)],
            );
            return stream.write_all(head.as_bytes()).await;
        }
        None => (0, size.saturating_sub(1)),
    };
    let length = if size == 0 {
        0
    } else {
        end.saturating_sub(start) + 1
    };
    let head = if range.is_some() {
        let content_range = format!("bytes {start}-{end}/{size}");
        response_head(
            206,
            "Partial Content",
            kind,
            length,
            &[("Content-Range", &content_range)],
        )
    } else {
        response_head(200, "OK", kind, length, &[])
    };
    stream.write_all(head.as_bytes()).await?;
    if head_only || length == 0 {
        return Ok(());
    }
    file.seek(SeekFrom::Start(start))?;
    let mut remaining = file.take(length);
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        let read = remaining.read(&mut chunk)?;
        if read == 0 {
            return Ok(());
        }
        stream.write_all(&chunk[..read]).await?;
    }
}

fn response_head(
    status: u16,
    reason: &str,
    kind: &str,
    length: u64,
    extra: &[(&str, &str)],
) -> String {
    let mut head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {kind}\r\nContent-Length: {length}\r\nCache-Control: no-store\r\nAccept-Ranges: bytes\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n"
    );
    for (name, value) in extra {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    head
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder() -> (tempfile::TempDir, Folder) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::write(root.join("index.html"), "<h1>hi</h1>").unwrap();
        std::fs::write(root.join(".env"), "SECRET=1").unwrap();
        std::fs::create_dir(root.join("assets")).unwrap();
        std::fs::write(root.join("assets/app one.js"), "1").unwrap();
        let folder = Folder {
            root,
            token: "t0k".into(),
            port: 4000,
        };
        (dir, folder)
    }

    fn get(folder: &Folder, target: &str) -> Answer {
        folder.answer(&Request::parse(&format!(
            "GET {target} HTTP/1.1\r\nHost: 127.0.0.1:4000\r\n\r\n"
        )))
    }

    #[test]
    fn navigate_targets_that_name_local_paths() {
        assert_eq!(
            local_target("file:///tmp/a%20b.html"),
            Some(PathBuf::from("/tmp/a b.html"))
        );
        assert_eq!(local_target("FILE:///tmp/x"), Some(PathBuf::from("/tmp/x")));
        assert_eq!(local_target(" /tmp/x "), Some(PathBuf::from("/tmp/x")));
        assert!(local_target("~/x").is_some_and(|path| path.ends_with("x") && path.is_absolute()));
        assert_eq!(local_target("https://example.com/"), None);
        assert_eq!(local_target("localhost:3000"), None);
        assert_eq!(local_target("relative/path.html"), None);
    }

    #[test]
    fn files_under_the_token_are_served_and_folders_fall_back_to_index() {
        let (_dir, folder) = folder();
        let root = folder.root.clone();
        assert_eq!(get(&folder, "/t0k/"), Answer::File(root.join("index.html")));
        assert_eq!(
            get(&folder, "/t0k/index.html?v=2#top"),
            Answer::File(root.join("index.html"))
        );
        assert_eq!(
            get(&folder, "/t0k/assets/app%20one.js"),
            Answer::File(root.join("assets/app one.js"))
        );
        assert_eq!(get(&folder, "/t0k"), Answer::Redirect("/t0k/".into()));
        assert_eq!(
            get(&folder, "/t0k/assets"),
            Answer::Redirect("/t0k/assets/".into())
        );
        let Answer::Page(listing) = get(&folder, "/t0k/assets/") else {
            panic!("a folder without index.html lists its files");
        };
        assert!(listing.contains("href=\"app%20one.js\""));
    }

    #[test]
    fn nothing_outside_the_folder_or_hidden_is_served() {
        let (_dir, folder) = folder();
        for target in [
            "/index.html",
            "/other/index.html",
            "/t0kx/index.html",
            "/t0k/../t0k/index.html",
            "/t0k/%2e%2e/etc/passwd",
            "/t0k/..%2Fetc%2Fpasswd",
            "/t0k/.env",
            "/t0k/%2Eenv",
            "/t0k/assets/%ZZ",
            "/t0k/missing.html",
        ] {
            assert_eq!(get(&folder, target), Answer::Refused(404), "{target}");
        }
    }

    #[test]
    fn a_symlink_out_of_the_folder_is_refused() {
        let (_dir, folder) = folder();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "x").unwrap();
        std::os::unix::fs::symlink(outside.path(), folder.root.join("link")).unwrap();
        assert_eq!(get(&folder, "/t0k/link/secret.txt"), Answer::Refused(404));
    }

    #[test]
    fn other_hosts_and_methods_are_refused() {
        let (_dir, folder) = folder();
        let rebound = Request::parse("GET /t0k/ HTTP/1.1\r\nHost: evil.test:4000\r\n\r\n");
        assert_eq!(folder.answer(&rebound), Answer::Refused(403));
        let hostless = Request::parse("GET /t0k/ HTTP/1.1\r\n\r\n");
        assert_eq!(folder.answer(&hostless), Answer::Refused(403));
        let post = Request::parse("POST /t0k/ HTTP/1.1\r\nHost: 127.0.0.1:4000\r\n\r\n");
        assert_eq!(folder.answer(&post), Answer::Refused(405));
        let localhost = Request::parse("GET /t0k/ HTTP/1.1\r\nHost: LOCALHOST:4000\r\n\r\n");
        assert_eq!(
            folder.answer(&localhost),
            Answer::File(folder.root.join("index.html"))
        );
    }

    #[test]
    fn the_home_folder_and_the_disk_root_are_too_broad() {
        let home = PathBuf::from(std::env::var("HOME").unwrap())
            .canonicalize()
            .unwrap();
        assert!(refuse_broad_root(Path::new("/")).is_err());
        assert!(refuse_broad_root(&home).is_err());
        assert!(refuse_broad_root(home.parent().unwrap()).is_err());
        assert!(refuse_broad_root(&home.join("project")).is_ok());
    }

    #[tokio::test]
    async fn a_shared_file_is_fetched_over_loopback() {
        let (dir, _) = folder();
        let files = LocalFiles::default();
        let url = files
            .url_for(&dir.path().join("assets/app one.js"))
            .unwrap();
        assert!(url.starts_with("http://127.0.0.1:"), "{url}");
        assert!(
            url.ends_with("/app%20one.js") || url.ends_with("/app one.js"),
            "{url}"
        );
        let parsed = Url::parse(&url).unwrap();
        let port = parsed.port().unwrap();
        let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let request = format!(
            "GET {} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nRange: bytes=0-\r\n\r\n",
            parsed.path()
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        assert!(response.starts_with("HTTP/1.1 206"), "{response}");
        assert!(
            response.contains("Content-Range: bytes 0-0/1"),
            "{response}"
        );
        assert!(response.ends_with("\r\n\r\n1"), "{response}");
        let same_folder = files.url_for(&dir.path().join("assets")).unwrap();
        assert_eq!(Url::parse(&same_folder).unwrap().port(), Some(port));
        assert!(same_folder.ends_with('/'), "{same_folder}");
        assert!(files.url_for(&dir.path().join(".env")).is_err());
        assert!(files.url_for(&dir.path().join("nope.html")).is_err());
    }
}
