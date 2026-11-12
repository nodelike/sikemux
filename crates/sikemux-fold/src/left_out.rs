//! Puts back the sentences an answer left out for resting on no source, for a reader who asked
//! to see them where they stood.

use serde_json::{json, Value};
use sikemux_view::chat::{ChatMessage, ChatPart, PartTree, RestoredPart, Unsourced};

/// Stands for a put-back sentence while the text is read again, and never leaves this module.
const PUT_BACK: &str = "sikemux://why/left-out";

/// Each text part of `msg` that had sentences left out, with them back where they stood: its tree,
/// with each of them an inline `faint` element.
pub fn restored_parts(msg: &ChatMessage) -> Vec<RestoredPart> {
    (msg.parts.iter())
        .filter_map(|part| {
            let ChatPart::Text { id, text, .. } = part else {
                return None;
            };
            let here: Vec<&Unsourced> = (msg.left_out.iter())
                .filter(|out| out.part == *id)
                .collect();
            if here.is_empty() {
                return None;
            }
            Some(RestoredPart {
                part: id.clone(),
                md: with_left_out(text, here),
            })
        })
        .collect()
}

/// The tree of `text` with the sentences `left_out` of it put back.
fn with_left_out(text: &str, mut left_out: Vec<&Unsourced>) -> PartTree {
    let mut restored = text.to_owned();
    left_out.sort_by_key(|out| std::cmp::Reverse(out.at));
    for out in left_out {
        let at = usize::try_from(out.at).unwrap_or(usize::MAX);
        if at > restored.len() || !restored.is_char_boundary(at) {
            continue;
        }
        let words = out.text.trim();
        let lead = out.text.len() - out.text.trim_start().len();
        let (Some(before), Some(after)) =
            (out.text.get(..lead), out.text.get(lead + words.len()..))
        else {
            continue;
        };
        let escaped = (words.replace('\\', "\\\\"))
            .replace('[', "\\[")
            .replace(']', "\\]");
        restored.insert_str(at, &format!("{before}[{escaped}]({PUT_BACK}){after}"));
    }
    let mut tree = crate::send::read(&restored);
    faint(&mut tree.blocks);
    tree
}

/// Turns each put-back sentence's link in `blocks` into faint words.
fn faint(blocks: &mut Value) {
    match blocks {
        Value::Array(items) => items.iter_mut().for_each(faint),
        Value::Object(element) => {
            let put_back = element.get("t").and_then(Value::as_str) == Some("a")
                && element.get("href").and_then(Value::as_str) == Some(PUT_BACK);
            if put_back {
                element.insert("t".into(), json!("faint"));
                element.remove("href");
            }
            if let Some(children) = element.get_mut("c") {
                faint(children);
            }
        }
        _ => {}
    }
}
