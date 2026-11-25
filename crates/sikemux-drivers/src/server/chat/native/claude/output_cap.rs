use std::collections::VecDeque;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use sikemux_process::sync::Locked;
use uuid::Uuid;

use super::super::json::text;

const TURN_BUDGET_CHARS: usize = 10_000;
const MIN_SLICE_CHARS: usize = 1_500;
const MOST_KEPT: usize = 32;
const SPILL_FOLDER: &str = "sikemux-tool-output-";
const LEFT_BEHIND: Duration = Duration::from_secs(24 * 60 * 60);

struct Kept {
    folder: PathBuf,
    files: Mutex<VecDeque<PathBuf>>,
}

static KEPT: LazyLock<Kept> = LazyLock::new(|| Kept {
    folder: std::env::temp_dir().join(format!("{SPILL_FOLDER}{}", Uuid::new_v4().simple())),
    files: Mutex::default(),
});

pub(super) fn cap_tool_output(used: &mut usize, input: &Value) -> Option<Value> {
    let mut subagent_used = 0usize;
    let budget = if input.get("agent_id").is_some() {
        &mut subagent_used
    } else {
        used
    };
    let tool = text(input, "tool_name")?;
    let response = input.get("tool_response")?;
    match tool {
        "Bash" => cap_bash(budget, response),
        "Read" => cap_read(budget, response),
        "WebFetch" => cap_web_fetch(budget, response),
        _ => {
            *budget = budget.saturating_add(approx_text_chars(response));
            None
        }
    }
}

fn allowed_chars(used: usize, total: usize) -> usize {
    if used.saturating_add(total) <= TURN_BUDGET_CHARS {
        total
    } else {
        TURN_BUDGET_CHARS.saturating_sub(used).max(MIN_SLICE_CHARS)
    }
}

fn cap_bash(used: &mut usize, response: &Value) -> Option<Value> {
    let obj = response.as_object()?;
    if obj.get("isImage").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let stdout = obj.get("stdout").and_then(Value::as_str).unwrap_or("");
    let stderr = obj.get("stderr").and_then(Value::as_str).unwrap_or("");
    let out_len = stdout.chars().count();
    let err_len = stderr.chars().count();
    let total = out_len + err_len;
    let allow = allowed_chars(*used, total);
    if total <= allow {
        *used = used.saturating_add(total);
        return None;
    }
    let full = if stderr.is_empty() {
        stdout.to_owned()
    } else if stdout.is_empty() {
        stderr.to_owned()
    } else {
        format!("{stdout}\n--- stderr ---\n{stderr}")
    };
    let saved = spill("bash", &full);
    let (out_allow, err_allow) = if err_len == 0 {
        (allow, 0)
    } else if out_len == 0 {
        (0, allow)
    } else {
        let out_share = ((allow as u128 * out_len as u128) / total as u128) as usize;
        let out_allow = out_share.clamp(300.min(allow), allow.saturating_sub(300));
        (out_allow, allow.saturating_sub(out_allow))
    };
    let mut modified = obj.clone();
    if out_len > out_allow {
        let omitted = out_len - out_allow;
        let note = spill_note(omitted, saved.as_ref());
        modified.insert("stdout".into(), json!(head_tail(stdout, out_allow, &note)));
    }
    if err_len > err_allow {
        let omitted = err_len - err_allow;
        let note = spill_note(omitted, saved.as_ref());
        modified.insert("stderr".into(), json!(head_tail(stderr, err_allow, &note)));
    }
    *used = used.saturating_add(allow);
    Some(Value::Object(modified))
}

fn cap_read(used: &mut usize, response: &Value) -> Option<Value> {
    let obj = response.as_object()?;
    if obj.get("type").and_then(Value::as_str) != Some("text") {
        return None;
    }
    let file = obj.get("file").and_then(Value::as_object)?;
    let content = file.get("content").and_then(Value::as_str)?;
    let total = content.chars().count();
    let allow = allowed_chars(*used, total);
    if total <= allow {
        *used = used.saturating_add(total);
        return None;
    }
    let omitted = total - allow;
    let note = match file.get("filePath").and_then(Value::as_str) {
        Some(path) => format!(
            "[... {omitted} chars omitted for per-turn output budget; read more of {path} with Read(offset, limit)]"
        ),
        None => format!(
            "[... {omitted} chars omitted for per-turn output budget; use Read(offset, limit) for more]"
        ),
    };
    let (trimmed, lines) = head_lines(content, allow, &note);
    let mut new_file = file.clone();
    new_file.insert("content".into(), json!(trimmed));
    if new_file.contains_key("numLines") {
        new_file.insert("numLines".into(), json!(lines));
    }
    let mut modified = obj.clone();
    modified.insert("file".into(), Value::Object(new_file));
    *used = used.saturating_add(allow);
    Some(Value::Object(modified))
}

fn cap_web_fetch(used: &mut usize, response: &Value) -> Option<Value> {
    let obj = response.as_object()?;
    let result = obj.get("result").and_then(Value::as_str)?;
    let total = result.chars().count();
    let allow = allowed_chars(*used, total);
    if total <= allow {
        *used = used.saturating_add(total);
        return None;
    }
    let saved = spill("webfetch", result);
    let note = spill_note(total - allow, saved.as_ref());
    let mut modified = obj.clone();
    modified.insert("result".into(), json!(head_tail(result, allow, &note)));
    *used = used.saturating_add(allow);
    Some(Value::Object(modified))
}

fn approx_text_chars(value: &Value) -> usize {
    match value {
        Value::String(text) => text.chars().count().min(TURN_BUDGET_CHARS),
        Value::Array(items) => items
            .iter()
            .filter(|item| item.get("type").and_then(Value::as_str) != Some("image"))
            .map(approx_text_chars)
            .sum::<usize>()
            .min(TURN_BUDGET_CHARS),
        Value::Object(fields) => {
            if fields.get("type").and_then(Value::as_str) == Some("image") {
                return 0;
            }
            fields
                .iter()
                .filter(|(k, _)| *k != "data")
                .map(|(_, v)| approx_text_chars(v))
                .sum::<usize>()
                .min(TURN_BUDGET_CHARS)
        }
        _ => 0,
    }
}

fn spill_note(omitted: usize, saved: Result<&PathBuf, &io::Error>) -> String {
    match saved {
        Ok(path) => format!(
            "[... {omitted} chars omitted for per-turn output budget; full output saved to {}]",
            path.display()
        ),
        Err(error) => {
            format!(
                "[... {omitted} chars omitted for per-turn output budget (save failed: {error})]"
            )
        }
    }
}

fn head_tail(text: &str, max_chars: usize, note: &str) -> String {
    let note_len = note.chars().count() + 2;
    let body = max_chars.saturating_sub(note_len).max(200);
    let head_n = (body * 3) / 5;
    let tail_n = body.saturating_sub(head_n);
    let head = take_prefix(text, head_n);
    let tail = take_suffix(text, tail_n);
    format!("{head}\n{note}\n{tail}")
}

fn head_lines(text: &str, max_chars: usize, note: &str) -> (String, usize) {
    let body = max_chars.saturating_sub(note.chars().count() + 1).max(200);
    let mut out = String::new();
    let mut kept = 0usize;
    let mut lines = 0usize;
    for line in text.split('\n') {
        let line_len = line.chars().count();
        let sep = usize::from(!out.is_empty());
        if kept + sep + line_len > body {
            if out.is_empty() {
                out.push_str(take_prefix(line, body));
                lines = 1;
            }
            break;
        }
        if !out.is_empty() {
            out.push('\n');
            kept += 1;
        }
        out.push_str(line);
        kept += line_len;
        lines += 1;
    }
    out.push('\n');
    out.push_str(note);
    (out, lines.saturating_add(1))
}

fn take_prefix(text: &str, chars: usize) -> &str {
    match text.char_indices().nth(chars) {
        Some((idx, _)) => text.get(..idx).unwrap_or(text),
        None => text,
    }
}

fn take_suffix(text: &str, chars: usize) -> &str {
    if chars == 0 {
        return "";
    }
    let total = text.chars().count();
    let skip = total.saturating_sub(chars);
    match text.char_indices().nth(skip) {
        Some((idx, _)) => text.get(idx..).unwrap_or(""),
        None => "",
    }
}

fn spill(prefix: &str, text: &str) -> io::Result<PathBuf> {
    let mut files = KEPT.files.locked();
    if files.is_empty() {
        clear_left_behind();
    }
    fs::create_dir_all(&KEPT.folder)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&KEPT.folder, fs::Permissions::from_mode(0o700));
    }
    let path = KEPT
        .folder
        .join(format!("{prefix}-{}.txt", Uuid::new_v4().simple()));
    write_private(&path, text.as_bytes())?;
    files.push_back(path.clone());
    while files.len() > MOST_KEPT {
        if let Some(oldest) = files.pop_front() {
            let _ = fs::remove_file(oldest);
        }
    }
    Ok(path)
}

fn write_private(path: &PathBuf, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)
}

fn clear_left_behind() {
    let Ok(entries) = fs::read_dir(std::env::temp_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with(SPILL_FOLDER) || entry.path() == KEPT.folder {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > LEFT_BEHIND);
        if stale {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}
