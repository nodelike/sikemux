//! Serves files the editor previews to the main window over `preview://`, so
//! pictures, media and documents load by URL instead of crossing IPC. Only a
//! file the window asked for through `preview_file` is served.

use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::Serialize;
use tauri::http::{header, Request, Response, StatusCode};
use tauri::{Manager, Runtime, State, UriSchemeContext, UriSchemeResponder};

use crate::error::{AppError, AppResult};
use crate::file_serving::{content_type, parse_range, percent_decode, sniff};

pub const SCHEME: &str = "preview";

/* A media element asks for the rest of the file from wherever it is reading;
answering in slices keeps a long video from being read into memory whole. */
const MAX_RANGE_BYTES: u64 = 1024 * 1024;
const SNIFF_BYTES: usize = 64;

#[derive(Default)]
pub struct Previews {
    granted: Mutex<HashSet<PathBuf>>,
}

#[derive(Serialize)]
pub struct FilePreview {
    mime: String,
    size: u64,
    /// Milliseconds since the epoch, so the window can tell a changed file from an unchanged one.
    modified: u64,
}

#[tauri::command]
pub async fn preview_file(previews: State<'_, Previews>, path: String) -> AppResult<FilePreview> {
    let path = PathBuf::from(path);
    let canonical = path.canonicalize()?;
    let metadata = canonical.metadata()?;
    if !metadata.is_file() {
        return Err(AppError::Fs(format!("{} is not a file", path.display())));
    }
    let mime = mime_of(&canonical)?;
    previews
        .granted
        .lock()
        .map_err(|_| AppError::Other("preview grants poisoned".into()))?
        .insert(canonical);
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_millis() as u64);
    Ok(FilePreview {
        mime: mime.into(),
        size: metadata.len(),
        modified,
    })
}

fn mime_of(path: &Path) -> std::io::Result<&'static str> {
    let named = content_type(path);
    if named != "application/octet-stream" {
        return Ok(named);
    }
    let mut head = Vec::with_capacity(SNIFF_BYTES);
    File::open(path)?
        .take(SNIFF_BYTES as u64)
        .read_to_end(&mut head)?;
    Ok(sniff(&head).unwrap_or(named))
}

pub fn handle<R: Runtime>(
    context: UriSchemeContext<'_, R>,
    request: Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    if context.webview_label() != "main" {
        responder.respond(status(StatusCode::FORBIDDEN));
        return;
    }
    let app = context.app_handle().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let previews = app.state::<Previews>();
        responder.respond(answer(&previews, &request));
    });
}

fn answer(previews: &Previews, request: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let Some(path) = request
        .uri()
        .path()
        .strip_prefix('/')
        .and_then(percent_decode)
        .and_then(|path| Path::new(&path).canonicalize().ok())
    else {
        return status(StatusCode::NOT_FOUND);
    };
    let granted = previews
        .granted
        .lock()
        .is_ok_and(|granted| granted.contains(&path));
    if !granted {
        return status(StatusCode::FORBIDDEN);
    }
    let range = request
        .headers()
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_range);
    serve(&path, range).unwrap_or_else(|_| status(StatusCode::NOT_FOUND))
}

fn serve(path: &Path, range: Option<(u64, Option<u64>)>) -> std::io::Result<Response<Vec<u8>>> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    let response = Response::builder()
        .header(header::CONTENT_TYPE, mime_of(path)?)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(header::ACCESS_CONTROL_EXPOSE_HEADERS, "Content-Range");
    let Some((start, end)) = range else {
        let mut body = Vec::with_capacity(size as usize);
        file.read_to_end(&mut body)?;
        return Ok(response.body(body).expect("static preview headers"));
    };
    if start >= size {
        return Ok(response
            .status(StatusCode::RANGE_NOT_SATISFIABLE)
            .header(header::CONTENT_RANGE, format!("bytes */{size}"))
            .body(Vec::new())
            .expect("static preview headers"));
    }
    let end = end
        .unwrap_or(size - 1)
        .min(size - 1)
        .min(start + MAX_RANGE_BYTES - 1);
    let mut body = Vec::with_capacity((end - start + 1) as usize);
    file.seek(SeekFrom::Start(start))?;
    file.take(end - start + 1).read_to_end(&mut body)?;
    Ok(response
        .status(StatusCode::PARTIAL_CONTENT)
        .header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{size}"))
        .body(body)
        .expect("static preview headers"))
}

fn status(code: StatusCode) -> Response<Vec<u8>> {
    Response::builder()
        .status(code)
        .body(Vec::new())
        .expect("a bare status response")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(previews: &Previews, path: &Path, range: Option<&str>) -> Response<Vec<u8>> {
        let encoded: String = path
            .to_string_lossy()
            .bytes()
            .map(|byte| format!("%{byte:02X}"))
            .collect();
        let mut request = Request::builder().uri(format!("preview://localhost/{encoded}"));
        if let Some(range) = range {
            request = request.header(header::RANGE, range);
        }
        answer(previews, &request.body(Vec::new()).unwrap())
    }

    fn grant(previews: &Previews, path: &Path) {
        previews
            .granted
            .lock()
            .unwrap()
            .insert(path.canonicalize().unwrap());
    }

    #[test]
    fn only_granted_files_are_served() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("clip.mp4");
        std::fs::write(&file, b"0123456789").unwrap();
        let previews = Previews::default();
        assert_eq!(get(&previews, &file, None).status(), StatusCode::FORBIDDEN);
        grant(&previews, &file);
        let response = get(&previews, &file, None);
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "video/mp4");
        assert_eq!(response.body(), b"0123456789");
    }

    #[test]
    fn ranges_answer_the_slice_asked_for() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("song.mp3");
        std::fs::write(&file, b"0123456789").unwrap();
        let previews = Previews::default();
        grant(&previews, &file);
        let response = get(&previews, &file, Some("bytes=2-5"));
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(response.headers()[header::CONTENT_RANGE], "bytes 2-5/10");
        assert_eq!(response.body(), b"2345");
        let open_ended = get(&previews, &file, Some("bytes=7-"));
        assert_eq!(open_ended.body(), b"789");
        let past_the_end = get(&previews, &file, Some("bytes=10-"));
        assert_eq!(past_the_end.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    }

    #[test]
    fn unnamed_files_are_typed_by_their_first_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("scan");
        std::fs::write(&file, b"%PDF-1.7\nrest").unwrap();
        assert_eq!(mime_of(&file).unwrap(), "application/pdf");
    }
}
