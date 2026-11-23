//! Mermaid diagrams in the pages agents show: a page that marks a block as `mermaid` gets
//! Mermaid itself and a script that draws each block in the window's theme, which pan and zoom.

use std::io::Read;
use std::sync::OnceLock;

use flate2::read::GzDecoder;

/// @mermaid-js/tiny 12.3.0's dist/mermaid.tiny.js, gzipped.
static MERMAID: &[u8] = include_bytes!("../../assets/mermaid.js.gz");
const DRAW: &str = include_str!("diagrams.js");
const HIDE_SOURCE: &str = "pre.mermaid:not([data-processed]){visibility:hidden}";

/// `html` with what draws its Mermaid blocks, when it has any.
pub(super) fn with_diagrams(html: String) -> String {
    if !has_diagram(&html) {
        return html;
    }
    let Some(mermaid) = mermaid() else {
        return html;
    };
    let markup =
        format!("<style>{HIDE_SOURCE}</style><script>{mermaid}</script><script>{DRAW}</script>");
    let lower = html.to_ascii_lowercase();
    let mut html = html;
    match lower.rfind("</body") {
        Some(at) => html.insert_str(at, &markup),
        None => html.push_str(&markup),
    }
    html
}

/// Whether a tag of `html` names `mermaid` or `language-mermaid` among its classes.
fn has_diagram(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    lower.match_indices("mermaid").any(|(at, _)| {
        let before = lower.get(..at).unwrap_or_default();
        let tag = before
            .rfind('<')
            .and_then(|open| before.get(open..))
            .unwrap_or_default();
        let Some((_, classes)) = tag.rsplit_once("class").filter(|_| !tag.contains('>')) else {
            return false;
        };
        let classes = classes
            .trim_start()
            .strip_prefix('=')
            .unwrap_or("")
            .trim_start();
        let mut quoted = classes.chars();
        quoted
            .next()
            .filter(|quote| *quote == '"' || *quote == '\'')
            .is_some_and(|quote| !quoted.as_str().contains(quote))
    })
}

fn mermaid() -> Option<&'static str> {
    static SCRIPT: OnceLock<Option<String>> = OnceLock::new();
    SCRIPT
        .get_or_init(|| {
            let mut script = String::new();
            GzDecoder::new(MERMAID).read_to_string(&mut script).ok()?;
            Some(script)
        })
        .as_deref()
}
