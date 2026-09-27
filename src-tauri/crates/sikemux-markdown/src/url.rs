const SAFE_SCHEMES: [&str; 6] = ["http", "https", "irc", "ircs", "mailto", "xmpp"];

fn is_drive_path(url: &str) -> bool {
    let mut chars = url.chars();
    matches!(
        (chars.next(), chars.next(), chars.next()),
        (Some(letter), Some(':'), Some('/' | '\\')) if letter.is_ascii_alphabetic()
    )
}

/// Empties a link whose scheme could run code, such as `javascript:`, and keeps
/// relative links, fragments and the web and mail schemes.
pub fn sanitize(url: &str, file_links: bool) -> String {
    if file_links
        && (url
            .get(..7)
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case("file://"))
            || is_drive_path(url))
    {
        return url.to_owned();
    }
    let Some(colon) = url.find(':') else {
        return url.to_owned();
    };
    let before_colon = |mark: char| url.find(mark).is_some_and(|at| at < colon);
    if before_colon('/') || before_colon('?') || before_colon('#') {
        return url.to_owned();
    }
    let scheme = url.get(..colon).unwrap_or_default();
    if SAFE_SCHEMES
        .iter()
        .any(|safe| safe.eq_ignore_ascii_case(scheme))
    {
        return url.to_owned();
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::sanitize;

    #[test]
    fn keeps_web_mail_and_relative_links() {
        for url in [
            "https://a.dev/x",
            "HTTP://a.dev",
            "mailto:a@b.dev",
            "xmpp:a@b.dev",
            "ircs://irc.libera.chat",
            "docs/a.md",
            "#usage",
            "?q=1:2",
            "./a:b",
        ] {
            assert_eq!(sanitize(url, false), url);
        }
    }

    #[test]
    fn empties_schemes_that_run_code() {
        for url in [
            "javascript:alert(1)",
            "JavaScript:alert(1)",
            "vbscript:x",
            "data:text/html,<b>x</b>",
            "file:///etc/passwd",
        ] {
            assert_eq!(sanitize(url, false), "");
        }
    }

    #[test]
    fn keeps_local_files_only_where_asked() {
        assert_eq!(
            sanitize("file:///Users/me/a.png", true),
            "file:///Users/me/a.png"
        );
        assert_eq!(sanitize("C:\\work\\a.rs", true), "C:\\work\\a.rs");
        assert_eq!(sanitize("C:/work/a.rs", true), "C:/work/a.rs");
        assert_eq!(sanitize("C:\\work\\a.rs", false), "");
        assert_eq!(sanitize("javascript:alert(1)", true), "");
    }
}
