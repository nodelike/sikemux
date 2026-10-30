//! Folds chats for tools that draw the app outside it, such as the showcase:
//! one JSON request per line of stdin, one JSON answer per line of stdout.
//! A request names its `chat`; `reset` starts it again, `events` are folded
//! and answered with the changes they made, `view` answers the chat as a
//! client opening it is sent, and `detail` what a tool call did.

use std::collections::HashMap;
use std::io::{self, BufRead, Write};

use serde::Deserialize;
use serde_json::{json, Value};
use sikemux_fold::{ChatEvent, Fold};

#[derive(Deserialize)]
struct Request {
    chat: String,
    #[serde(default)]
    reset: bool,
    #[serde(default)]
    events: Vec<ChatEvent>,
    #[serde(default)]
    view: bool,
    #[serde(default)]
    detail: Option<String>,
}

struct Chat {
    fold: Fold,
    next: u64,
    seq: u64,
}

fn main() -> io::Result<()> {
    let mut chats: HashMap<String, Chat> = HashMap::new();
    let mut out = io::stdout().lock();
    for line in io::stdin().lock().lines() {
        let request: Request = serde_json::from_str(&line?).map_err(io::Error::other)?;
        if request.reset {
            chats.remove(&request.chat);
        }
        let chat = chats.entry(request.chat).or_insert_with(|| Chat {
            fold: Fold::new(sikemux_wire::protocol::PermissionMode::WorkspaceWrite),
            next: 0,
            seq: 0,
        });
        let answer = if request.view {
            json!({ "view": chat.fold.view(false), "seq": chat.seq })
        } else if let Some(id) = request.detail {
            json!({ "detail": chat.fold.tool_detail(&id) })
        } else {
            for event in &request.events {
                let at = event
                    .payload
                    .get("at")
                    .and_then(Value::as_u64)
                    .unwrap_or_default();
                chat.fold.apply(chat.next, event.kind, &event.payload, at);
                chat.next += 1;
            }
            let changes = chat.fold.flush();
            if !changes.is_empty() {
                chat.seq += 1;
                chat.fold.number(chat.seq + 1);
            }
            json!({ "changes": changes, "seq": chat.seq })
        };
        serde_json::to_writer(&mut out, &answer).map_err(io::Error::other)?;
        out.write_all(b"\n")?;
        out.flush()?;
    }
    Ok(())
}
