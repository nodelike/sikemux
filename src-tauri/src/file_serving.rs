//! What a local file is served as, shared by the editor's preview scheme and
//! the loopback server that shows local files in browser tabs.

use std::path::Path;

pub fn content_type(path: &Path) -> &'static str {
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" | "map" => "application/json",
        "txt" | "md" | "csv" | "log" => "text/plain; charset=utf-8",
        "xml" => "application/xml",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "tif" | "tiff" => "image/tiff",
        "heic" => "image/heic",
        "heif" => "image/heif",
        "psd" => "image/vnd.adobe.photoshop",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "wasm" => "application/wasm",
        "pdf" => "application/pdf",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "mkv" => "video/x-matroska",
        "avi" => "video/x-msvideo",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "aac" => "audio/aac",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "aif" | "aiff" => "audio/aiff",
        "ogg" | "oga" | "opus" => "audio/ogg",
        "rtf" => "application/rtf",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "odt" => "application/vnd.oasis.opendocument.text",
        "ods" => "application/vnd.oasis.opendocument.spreadsheet",
        "odp" => "application/vnd.oasis.opendocument.presentation",
        "pages" => "application/vnd.apple.pages",
        "numbers" => "application/vnd.apple.numbers",
        "key" => "application/vnd.apple.keynote",
        "epub" => "application/epub+zip",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

/// The type a file's first bytes announce, for files whose name does not say.
pub fn sniff(head: &[u8]) -> Option<&'static str> {
    let starts = |magic: &[u8]| head.starts_with(magic);
    let at = |offset: usize, magic: &[u8]| head.get(offset..offset + magic.len()) == Some(magic);
    if starts(b"%PDF-") {
        return Some("application/pdf");
    }
    if starts(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if starts(b"\xff\xd8\xff") {
        return Some("image/jpeg");
    }
    if starts(b"GIF87a") || starts(b"GIF89a") {
        return Some("image/gif");
    }
    if starts(b"RIFF") {
        return match head.get(8..12) {
            Some(b"WEBP") => Some("image/webp"),
            Some(b"WAVE") => Some("audio/wav"),
            Some(b"AVI ") => Some("video/x-msvideo"),
            _ => None,
        };
    }
    if at(4, b"ftyp") {
        return Some(match head.get(8..12) {
            Some(b"heic" | b"heix" | b"mif1" | b"msf1") => "image/heic",
            Some(b"avif") => "image/avif",
            Some(b"M4A " | b"M4B ") => "audio/mp4",
            Some(b"qt  ") => "video/quicktime",
            _ => "video/mp4",
        });
    }
    if starts(b"ID3") || starts(b"\xff\xfb") || starts(b"\xff\xf3") || starts(b"\xff\xf2") {
        return Some("audio/mpeg");
    }
    if starts(b"fLaC") {
        return Some("audio/flac");
    }
    if starts(b"OggS") {
        return Some("audio/ogg");
    }
    if starts(b"FORM") && (at(8, b"AIFF") || at(8, b"AIFC")) {
        return Some("audio/aiff");
    }
    if starts(b"\x1a\x45\xdf\xa3") {
        return Some("video/webm");
    }
    if starts(b"wOFF") {
        return Some("font/woff");
    }
    if starts(b"wOF2") {
        return Some("font/woff2");
    }
    if starts(b"OTTO") {
        return Some("font/otf");
    }
    if starts(b"\x00\x01\x00\x00\x00") {
        return Some("font/ttf");
    }
    if starts(b"8BPS") {
        return Some("image/vnd.adobe.photoshop");
    }
    if starts(b"II*\x00") || starts(b"MM\x00*") {
        return Some("image/tiff");
    }
    if starts(b"{\\rtf") {
        return Some("application/rtf");
    }
    if starts(b"PK\x03\x04") {
        return Some("application/zip");
    }
    None
}

/// A single `bytes=start-` or `bytes=start-end` range, which is all media elements ask for.
pub fn parse_range(value: &str) -> Option<(u64, Option<u64>)> {
    let (start, end) = value.strip_prefix("bytes=")?.split_once('-')?;
    let start = start.trim().parse().ok()?;
    let end = match end.trim() {
        "" => None,
        end => Some(end.parse().ok()?),
    };
    Some((start, end))
}

pub fn percent_decode(segment: &str) -> Option<String> {
    let bytes = segment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            let hex = segment.get(at + 1..at + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_decide_the_type() {
        assert_eq!(content_type(Path::new("/a/b.PDF")), "application/pdf");
        assert_eq!(content_type(Path::new("/a/song.m4a")), "audio/mp4");
        assert_eq!(
            content_type(Path::new("/a/noext")),
            "application/octet-stream"
        );
    }

    #[test]
    fn first_bytes_decide_the_type_of_unnamed_files() {
        assert_eq!(sniff(b"%PDF-1.7\n"), Some("application/pdf"));
        assert_eq!(sniff(b"\0\0\0\x20ftypheic"), Some("image/heic"));
        assert_eq!(sniff(b"\0\0\0\x20ftypisom"), Some("video/mp4"));
        assert_eq!(sniff(b"RIFF\0\0\0\0WAVEfmt "), Some("audio/wav"));
        assert_eq!(sniff(b"ID3\x04"), Some("audio/mpeg"));
        assert_eq!(sniff(b"\x7fELF"), None);
        assert_eq!(sniff(b""), None);
    }

    #[test]
    fn media_ranges_parse() {
        assert_eq!(parse_range("bytes=0-"), Some((0, None)));
        assert_eq!(parse_range("bytes=10-19"), Some((10, Some(19))));
        assert_eq!(parse_range("bytes=-5"), None);
        assert_eq!(parse_range("items=0-1"), None);
    }

    #[test]
    fn percent_escapes_decode() {
        assert_eq!(percent_decode("a%20b"), Some("a b".into()));
        assert_eq!(percent_decode("%2FUsers%2Fme"), Some("/Users/me".into()));
        assert_eq!(percent_decode("%zz"), None);
    }
}
