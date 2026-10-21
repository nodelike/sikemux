use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use super::super::super::states::{self, Subagent};
use super::super::json::{self, is_js_whitespace, js_str, str_field, truthy};
use super::failure;
use super::tools::{self, TaskPlan};

/// Reads the active branch of a transcript: follow parentUuid back from the LAST record that is user/assistant (and not isSidechain), producing the ordered chain of records.
///
/// This is the SDK's `getSessionMessages`: compaction boundaries relink their preserved
/// messages, sibling records of a split assistant message (parallel tool calls) and their
/// tool results are put back after it, queued prompts become user records, and meta,
/// sidechain and team records are dropped. Records are returned as stored, so they carry no
/// `parent_tool_use_id`; Claude Code keeps subagent transcripts in separate files.
pub fn read_chain(path: &Path) -> Result<Vec<Value>, String> {
    read(path, false)
}

/// The active branch back to the chat's first message, where `read_chain` starts
/// at its last compaction.
pub fn read_history(path: &Path) -> Result<Vec<Value>, String> {
    read(path, true)
}

fn read(path: &Path, across_compactions: bool) -> Result<Vec<Value>, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let entries = parse_entries(&bytes);
    let chain = active_branch(entries, across_compactions);
    Ok(finish_messages(chain))
}

/// `records` with each subagent's own transcript put in after the call that
/// spawned it, so a loaded chat shows what its subagents did. Claude Code
/// keeps those beside the chat's, in `<session>/subagents/agent-<id>.jsonl`,
/// each with a `.meta.json` naming the spawning call.
pub fn with_subagents(path: &Path, records: Vec<Value>) -> Vec<Value> {
    let dir = path.with_extension("").join("subagents");
    let Ok(listing) = std::fs::read_dir(&dir) else {
        return records;
    };
    let metas: Vec<PathBuf> = listing
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.to_str()
                .is_some_and(|name| name.ends_with(".meta.json"))
        })
        .collect();
    let threads = std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(metas.len().max(1));
    let read: Vec<(String, String, Vec<Value>)> = std::thread::scope(|scope| {
        let reading: Vec<_> = metas
            .chunks(metas.len().div_ceil(threads).max(1))
            .map(|metas| {
                scope.spawn(|| {
                    metas
                        .iter()
                        .filter_map(|meta| subagent(meta))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        reading
            .into_iter()
            .flat_map(|read| read.join().unwrap_or_default())
            .collect()
    });
    let mut by_spawner: HashMap<String, Vec<(String, Vec<Value>)>> = HashMap::new();
    for (spawner, started, chain) in read {
        by_spawner
            .entry(spawner)
            .or_default()
            .push((started, chain));
    }
    let mut out = Vec::with_capacity(records.len());
    put_in_subagents(records, &mut by_spawner, &mut out);
    out
}

/// A subagent's transcript, beside the call that spawned it and when it started.
fn subagent(meta_path: &Path) -> Option<(String, String, Vec<Value>)> {
    let stem = meta_path.to_str()?.strip_suffix(".meta.json")?;
    let meta: Value = serde_json::from_slice(&std::fs::read(meta_path).ok()?).ok()?;
    let spawner = str_field(&meta, "toolUseId")?.to_owned();
    let bytes = std::fs::read(format!("{stem}.jsonl")).ok()?;
    let mut entries = parse_entries(&bytes);
    for entry in &mut entries {
        entry["isSidechain"] = Value::Bool(false);
    }
    let mut chain = finish_messages(active_branch(entries, true));
    // The task it was handed opens its transcript, and the subagent's
    // own row already says it.
    if chain.first().and_then(|record| str_field(record, "type")) == Some("user") {
        chain.remove(0);
    }
    for record in &mut chain {
        record["parent_tool_use_id"] = Value::String(spawner.clone());
    }
    let started = chain
        .first()
        .and_then(|record| str_field(record, "timestamp"))
        .unwrap_or_default()
        .to_owned();
    Some((spawner, started, chain))
}

/// `records` into `out`, each followed by the transcripts of the subagents it
/// spawned, and theirs by the subagents those spawned.
fn put_in_subagents(
    records: Vec<Value>,
    by_spawner: &mut HashMap<String, Vec<(String, Vec<Value>)>>,
    out: &mut Vec<Value>,
) {
    for record in records {
        let mut spawned: Vec<(String, Vec<Value>)> = tool_use_ids(&record)
            .filter_map(|id| by_spawner.remove(id))
            .flatten()
            .collect();
        out.push(record);
        spawned.sort_by(|a, b| a.0.cmp(&b.0));
        for (_, chain) in spawned {
            put_in_subagents(chain, by_spawner, out);
        }
    }
}

fn tool_use_ids(record: &Value) -> impl Iterator<Item = &str> {
    record
        .pointer("/message/content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|block| str_field(block, "type") == Some("tool_use"))
        .filter_map(|block| str_field(block, "id"))
}

/// A transcript bigger than this is read on several threads, in pieces of
/// about this size.
const PIECE_BYTES: usize = 8 * 1024 * 1024;

fn parse_entries(bytes: &[u8]) -> Vec<Value> {
    let threads = std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(bytes.len() / PIECE_BYTES + 1);
    if threads <= 1 {
        return parse_lines(bytes);
    }
    let mut pieces = Vec::with_capacity(threads);
    let mut start = 0;
    for piece in 1..threads {
        let mut end = (bytes.len() * piece / threads).max(start);
        while end < bytes.len() && bytes[end] != b'\n' {
            end += 1;
        }
        pieces.push(&bytes[start..end]);
        start = end;
    }
    pieces.push(&bytes[start..]);
    std::thread::scope(|scope| {
        let parsing: Vec<_> = pieces
            .into_iter()
            .map(|piece| scope.spawn(move || parse_lines(piece)))
            .collect();
        parsing
            .into_iter()
            .flat_map(|parsed| parsed.join().unwrap_or_default())
            .collect()
    })
}

fn parse_lines(bytes: &[u8]) -> Vec<Value> {
    bytes
        .split(|byte| *byte == b'\n')
        .filter_map(|line| {
            let start = line.iter().position(|byte| *byte > b' ')?;
            let text = String::from_utf8_lossy(&line[start..]);
            serde_json::from_str::<Value>(&text).ok()
        })
        .filter(|record| {
            matches!(
                str_field(record, "type"),
                Some("user" | "assistant" | "progress" | "system" | "attachment")
            ) && record.get("uuid").is_some_and(Value::is_string)
        })
        .collect()
}

fn uuid_of(record: &Value) -> &str {
    str_field(record, "uuid").unwrap_or_default()
}

fn is_conversation(record: &Value) -> bool {
    matches!(str_field(record, "type"), Some("user" | "assistant"))
}

fn set_parent(record: &mut Value, parent: Option<Value>) {
    if let Some(map) = record.as_object_mut() {
        match parent {
            Some(parent) => {
                map.insert("parentUuid".into(), parent);
            }
            None => {
                map.remove("parentUuid");
            }
        }
    }
}

/// Records by uuid, kept in first-seen order with the last copy of each uuid winning.
struct Records {
    order: Vec<String>,
    by_uuid: HashMap<String, Value>,
}

impl Records {
    fn new(entries: &[Value]) -> Self {
        let mut order = Vec::new();
        let mut by_uuid = HashMap::new();
        for entry in entries {
            let uuid = uuid_of(entry).to_string();
            if by_uuid.insert(uuid.clone(), entry.clone()).is_none() {
                order.push(uuid);
            }
        }
        Self { order, by_uuid }
    }

    fn get(&self, uuid: &str) -> Option<&Value> {
        self.by_uuid.get(uuid)
    }

    fn parent_of(&self, record: &Value) -> Option<&Value> {
        record
            .get("parentUuid")
            .filter(|parent| truthy(Some(parent)))
            .and_then(Value::as_str)
            .and_then(|parent| self.get(parent))
    }

    /// The last message before a compaction, which its boundary names only
    /// as a logical parent.
    fn compacted_from(&self, record: &Value) -> Option<&Value> {
        if str_field(record, "subtype") != Some("compact_boundary") {
            return None;
        }
        self.get(str_field(record, "logicalParentUuid")?)
    }

    fn reparent_where(&mut self, from: Option<&Value>, except: &str, to: Option<Value>) {
        for uuid in &self.order {
            if uuid == except {
                continue;
            }
            if let Some(record) = self.by_uuid.get_mut(uuid) {
                if record.get("parentUuid") == from {
                    set_parent(record, to.clone());
                }
            }
        }
    }

    fn relink_compactions(&mut self) {
        let boundaries: Vec<Value> = self
            .order
            .iter()
            .filter_map(|uuid| self.by_uuid.get(uuid))
            .filter(|record| {
                str_field(record, "type") == Some("system")
                    && str_field(record, "subtype") == Some("compact_boundary")
            })
            .map(|record| {
                record
                    .get("compactMetadata")
                    .cloned()
                    .unwrap_or(Value::Null)
            })
            .collect();
        for metadata in boundaries {
            let preserved = metadata
                .get("preservedMessages")
                .filter(|p| truthy(Some(p)));
            if let Some(preserved) = preserved {
                self.relink_preserved_messages(preserved);
            } else if let Some(segment) =
                metadata.get("preservedSegment").filter(|s| truthy(Some(s)))
            {
                self.relink_preserved_segment(segment);
            }
        }
    }

    fn relink_preserved_messages(&mut self, preserved: &Value) {
        let Some(uuids) = preserved.get("uuids").and_then(Value::as_array) else {
            return;
        };
        let uuids: Option<Vec<&str>> = uuids.iter().map(Value::as_str).collect();
        let Some(uuids) = uuids else {
            return;
        };
        if uuids.is_empty() || uuids.iter().any(|uuid| self.get(uuid).is_none()) {
            return;
        }
        let anchor = preserved.get("anchorUuid").cloned();
        let mut previous = anchor.clone();
        for uuid in &uuids {
            if let Some(record) = self.by_uuid.get_mut(*uuid) {
                set_parent(record, previous.clone());
            }
            previous = Some(Value::String((*uuid).to_string()));
        }
        let (Some(first), Some(last)) = (uuids.first().copied(), uuids.last().copied()) else {
            return;
        };
        self.reparent_where(
            anchor.as_ref(),
            first,
            Some(Value::String(last.to_string())),
        );
    }

    fn relink_preserved_segment(&mut self, segment: &Value) {
        let anchor = segment.get("anchorUuid").cloned();
        let head = str_field(segment, "headUuid")
            .unwrap_or_default()
            .to_string();
        if let Some(record) = self.by_uuid.get_mut(&head) {
            set_parent(record, anchor.clone());
        }
        self.reparent_where(anchor.as_ref(), &head, segment.get("tailUuid").cloned());
    }
}

fn active_branch(entries: Vec<Value>, across_compactions: bool) -> Vec<Value> {
    let mut records = Records::new(&entries);
    // Before a compaction the messages it kept already stand in their place.
    if !across_compactions {
        records.relink_compactions();
    }
    let mut file_index: HashMap<&str, usize> = HashMap::new();
    for (index, entry) in entries.iter().enumerate() {
        file_index.insert(uuid_of(entry), index);
    }
    let parents: HashSet<&str> = records
        .order
        .iter()
        .filter_map(|uuid| records.get(uuid))
        .filter_map(|record| record.get("parentUuid").filter(|p| truthy(Some(p))))
        .filter_map(Value::as_str)
        .collect();
    let mut tips: Vec<&Value> = Vec::new();
    for uuid in records
        .order
        .iter()
        .filter(|uuid| !parents.contains(uuid.as_str()))
    {
        let mut seen = HashSet::new();
        let mut current = records.get(uuid);
        while let Some(record) = current {
            if !seen.insert(uuid_of(record)) {
                break;
            }
            if is_conversation(record) {
                tips.push(record);
                break;
            }
            current = records.parent_of(record);
        }
    }
    let main_tips: Vec<&Value> = tips
        .iter()
        .copied()
        .filter(|record| {
            !truthy(record.get("isSidechain"))
                && !truthy(record.get("teamName"))
                && !truthy(record.get("isMeta"))
        })
        .collect();
    let pool = if main_tips.is_empty() {
        &tips
    } else {
        &main_tips
    };
    let rank = |record: &Value| file_index.get(uuid_of(record)).map_or(-1, |i| *i as i64);
    let Some(tip) = pool
        .iter()
        .copied()
        .reduce(|best, next| if rank(next) > rank(best) { next } else { best })
    else {
        return Vec::new();
    };
    let mut chain = Vec::new();
    let mut on_chain = HashSet::new();
    let mut current = records.get(uuid_of(tip));
    while let Some(record) = current {
        if !on_chain.insert(uuid_of(record).to_string()) {
            break;
        }
        chain.push(record);
        current = records.parent_of(record).or_else(|| {
            across_compactions
                .then(|| records.compacted_from(record))
                .flatten()
        });
    }
    chain.reverse();
    restore_split_messages(&records, chain, on_chain)
}

fn api_message_id(record: &Value) -> Option<&str> {
    if str_field(record, "type") != Some("assistant") {
        return None;
    }
    record.get("message")?.get("id")?.as_str()
}

fn carries_tool_result(record: &Value) -> bool {
    str_field(record, "type") == Some("user")
        && truthy(record.get("parentUuid"))
        && record
            .get("message")
            .and_then(|message| message.get("content"))
            .and_then(Value::as_array)
            .is_some_and(|blocks| {
                blocks
                    .iter()
                    .any(|block| str_field(block, "type") == Some("tool_result"))
            })
}

/// Claude stores each content block of one API message as its own record; parallel tool
/// calls branch, so the chain holds only one of them. Put the others, and their results,
/// back after the message's last record on the chain.
fn restore_split_messages(
    records: &Records,
    chain: Vec<&Value>,
    mut on_chain: HashSet<String>,
) -> Vec<Value> {
    let assistants: Vec<&Value> = chain
        .iter()
        .copied()
        .filter(|record| str_field(record, "type") == Some("assistant"))
        .collect();
    if assistants.is_empty() {
        return chain.into_iter().cloned().collect();
    }
    let mut last_on_chain: HashMap<&str, &str> = HashMap::new();
    for record in &assistants {
        if let Some(id) = api_message_id(record) {
            last_on_chain.insert(id, uuid_of(record));
        }
    }
    let mut by_message: HashMap<&str, Vec<&Value>> = HashMap::new();
    let mut results_by_parent: HashMap<&str, Vec<&Value>> = HashMap::new();
    for record in records.order.iter().filter_map(|uuid| records.get(uuid)) {
        if let Some(id) = api_message_id(record) {
            by_message.entry(id).or_default().push(record);
        } else if carries_tool_result(record) {
            if let Some(parent) = str_field(record, "parentUuid") {
                results_by_parent.entry(parent).or_default().push(record);
            }
        }
    }
    let by_time = |a: &&Value, b: &&Value| {
        str_field(a, "timestamp")
            .unwrap_or_default()
            .cmp(str_field(b, "timestamp").unwrap_or_default())
    };
    let mut seen_messages = HashSet::new();
    let mut extras: HashMap<&str, Vec<&Value>> = HashMap::new();
    let mut restored = 0;
    for record in &assistants {
        let Some(id) = api_message_id(record) else {
            continue;
        };
        if !seen_messages.insert(id) {
            continue;
        }
        let siblings = by_message.get(id).cloned().unwrap_or_else(|| vec![*record]);
        let mut parts: Vec<&Value> = siblings
            .iter()
            .copied()
            .filter(|sibling| !on_chain.contains(uuid_of(sibling)))
            .collect();
        let mut results: Vec<&Value> = siblings
            .iter()
            .filter_map(|sibling| results_by_parent.get(uuid_of(sibling)))
            .flatten()
            .copied()
            .filter(|result| !on_chain.contains(uuid_of(result)))
            .collect();
        if parts.is_empty() && results.is_empty() {
            continue;
        }
        parts.sort_by(by_time);
        results.sort_by(by_time);
        parts.extend(results);
        for part in &parts {
            on_chain.insert(uuid_of(part).to_string());
        }
        restored += parts.len();
        if let Some(anchor) = last_on_chain.get(id) {
            extras.insert(anchor, parts);
        }
    }
    if restored == 0 {
        return chain.into_iter().cloned().collect();
    }
    let mut out = Vec::new();
    for record in chain {
        out.push(record.clone());
        if let Some(parts) = extras.get(uuid_of(record)) {
            out.extend(parts.iter().map(|part| (*part).clone()));
        }
    }
    out
}

#[derive(PartialEq, Clone, Copy)]
enum LocalCommandPart {
    Record,
    Output,
    Caveat,
}

fn local_command_part(record: &Value) -> Option<LocalCommandPart> {
    if record.get("promptSource").is_some() {
        return None;
    }
    let content = record.get("message")?.get("content")?;
    let text = match content {
        Value::String(text) => text.as_str(),
        Value::Array(blocks) => blocks
            .iter()
            .rev()
            .find(|block| str_field(block, "type") == Some("text"))?
            .get("text")?
            .as_str()?,
        _ => return None,
    };
    [
        ("<command-name>", LocalCommandPart::Record),
        ("<local-command-stdout>", LocalCommandPart::Output),
        ("<local-command-stderr>", LocalCommandPart::Output),
        ("<local-command-caveat>", LocalCommandPart::Caveat),
    ]
    .into_iter()
    .find_map(|(open, part)| text.starts_with(open).then_some(part))
}

const INTERRUPTION_PREFIXES: [&str; 8] = [
    "[Request interrupted by user]",
    "[Request interrupted by user for tool use]",
    "[Tool call did not complete: the turn was ended to deliver the message that follows. Nothing refused it; re-run it if still needed.]",
    "[Tool call interrupted: the session ended before this call's result was recorded, so its outcome is unknown. Check whether it took effect before relying on it or running it again.]",
    "[Tool call result not in this copy: this session was copied from another session before that session recorded this call's result. The call may have finished there, may still be running there, or may never have run. Check whether it took effect before relying on it or running it again.]",
    "The user doesn't want to take this action right now. STOP what you are doing and wait for the user to tell you how to proceed.",
    "[Tool call skipped: the turn was stopped before this call ran, by the check whose denial is on another call in this batch. Nothing refused this call and it had no effects; re-run it if still needed.]",
    "[Tool call skipped: the turn ended to deliver the message that follows before this call ran. Nothing refused it; re-run it if still needed.]",
];

fn is_interruption(record: &Value) -> bool {
    if str_field(record, "type") != Some("user") {
        return false;
    }
    let starts = |text: &str| INTERRUPTION_PREFIXES.iter().any(|p| text.starts_with(p));
    match record.get("message").and_then(|m| m.get("content")) {
        Some(Value::String(text)) => starts(text),
        Some(Value::Array(blocks)) => {
            !blocks.is_empty()
                && blocks.iter().all(|block| {
                    let text = match str_field(block, "type") {
                        Some("text") => block.get("text"),
                        Some("tool_result")
                            if block.get("is_error") == Some(&Value::Bool(true)) =>
                        {
                            block.get("content")
                        }
                        _ => None,
                    };
                    text.and_then(Value::as_str).is_some_and(starts)
                })
        }
        _ => false,
    }
}

/// Indices of the command and output records that follow a local-command caveat.
fn completed_local_commands(records: &[Value]) -> HashSet<usize> {
    let mut completed = HashSet::new();
    for (index, record) in records.iter().enumerate() {
        if str_field(record, "type") != Some("user")
            || !truthy(record.get("isMeta"))
            || local_command_part(record) != Some(LocalCommandPart::Caveat)
        {
            continue;
        }
        let mut seen_command = false;
        for (later_index, later) in records.iter().enumerate().skip(index + 1) {
            match str_field(later, "type") {
                Some("assistant") => break,
                Some("user") if !truthy(later.get("isMeta")) => {}
                _ => continue,
            }
            match local_command_part(later) {
                Some(LocalCommandPart::Record) if !seen_command => seen_command = true,
                Some(LocalCommandPart::Output) if seen_command => {}
                _ => break,
            }
            completed.insert(later_index);
        }
    }
    completed
}

/// For each record, whether the next prompt-or-reply after it is a reply.
fn answered_flags(records: &[Value], completed: &HashSet<usize>) -> Vec<bool> {
    let mut flags = vec![false; records.len()];
    let mut next_is_reply = false;
    for (index, record) in records.iter().enumerate().rev() {
        flags[index] = next_is_reply;
        if str_field(record, "type") == Some("assistant")
            || carries_tool_result(record)
            || is_interruption(record)
        {
            next_is_reply = true;
        } else if str_field(record, "type") == Some("user")
            && !truthy(record.get("isMeta"))
            && !truthy(record.get("isCompactSummary"))
            && !completed.contains(&index)
        {
            next_is_reply = false;
        }
    }
    flags
}

fn queued_prompt(record: &Value, uuids: &mut HashSet<String>) -> Option<Value> {
    if str_field(record, "type") != Some("attachment") {
        return None;
    }
    let attachment = record.get("attachment").filter(|a| a.is_object())?;
    if str_field(attachment, "type") != Some("queued_command") || truthy(attachment.get("isMeta")) {
        return None;
    }
    let prompt = attachment
        .get("prompt")
        .filter(|prompt| prompt.is_string() || prompt.is_array())?;
    let forwarded = attachment
        .get("forwardedIntent")
        .and_then(|intent| intent.get("lineage"))
        .and_then(Value::as_str)
        .is_some_and(|lineage| !lineage.is_empty());
    if forwarded {
        return None;
    }
    let uuid = str_field(attachment, "source_uuid")
        .filter(|uuid| !uuid.is_empty())
        .unwrap_or(uuid_of(record))
        .to_string();
    if uuid != uuid_of(record) && uuids.contains(&uuid) {
        return None;
    }
    uuids.insert(uuid.clone());
    let mut user = Map::new();
    user.insert("type".into(), "user".into());
    user.insert("uuid".into(), uuid.into());
    for key in ["parentUuid", "sessionId", "timestamp"] {
        if let Some(value) = record.get(key) {
            user.insert(key.into(), value.clone());
        }
    }
    user.insert(
        "message".into(),
        json!({ "role": "user", "content": prompt }),
    );
    user.insert("isMeta".into(), false.into());
    if let Some(origin) = attachment
        .get("origin")
        .filter(|origin| origin.get("kind").is_some_and(Value::is_string))
    {
        user.insert("origin".into(), origin.clone());
    }
    user.insert("isQueuedCommand".into(), true.into());
    for key in ["isSidechain", "teamName"] {
        if let Some(value) = record.get(key) {
            user.insert(key.into(), value.clone());
        }
    }
    Some(Value::Object(user))
}

fn finish_messages(records: Vec<Value>) -> Vec<Value> {
    let completed = completed_local_commands(&records);
    let answered = answered_flags(&records, &completed);
    let mut uuids: HashSet<String> = records.iter().map(|r| uuid_of(r).to_string()).collect();
    records
        .into_iter()
        .enumerate()
        .map(|(index, mut record)| {
            if completed.contains(&index) {
                if let Some(map) = record.as_object_mut() {
                    map.insert("isCompletedLocalCommand".into(), true.into());
                }
                return record;
            }
            if answered[index] {
                if let Some(user) = queued_prompt(&record, &mut uuids) {
                    return user;
                }
            }
            record
        })
        .filter(|record| {
            is_conversation(record)
                && !truthy(record.get("isMeta"))
                && !truthy(record.get("isSidechain"))
                && !truthy(record.get("teamName"))
        })
        .collect()
}

/// The ids of the messages the transcript holds as sent by the person.
pub fn user_message_ids(records: &[Value]) -> impl Iterator<Item = String> + '_ {
    records
        .iter()
        .filter(|record| {
            str_field(record, "type") == Some("user") && record.get("toolUseResult").is_none()
        })
        .filter_map(|record| str_field(record, "uuid"))
        .map(str::to_owned)
}

/// The model of the last top-level non-synthetic assistant message, for the model picker.
pub fn resumed_model(records: &[Value]) -> Option<String> {
    records.iter().rev().find_map(|record| {
        let top_level = str_field(record, "type") == Some("assistant")
            && record.get("parent_tool_use_id").is_none_or(Value::is_null)
            && record.get("parent_agent_id").is_none_or(Value::is_null);
        if !top_level {
            return None;
        }
        let model = record
            .get("message")
            .filter(|message| message.is_object())?
            .get("model")?
            .as_str()?
            .trim_matches(is_js_whitespace);
        let placeholder = model.len() > 2
            && model.starts_with('<')
            && model.ends_with('>')
            && !model[1..model.len() - 1].contains('>');
        (!model.is_empty() && !placeholder).then(|| model.to_string())
    })
}

/// The updates to replay, in order, as (session_id, update) pairs: root session id for top-level records, `<sid>:replay-subagent:<toolUseId>` children with subagent_spawned / subagent_state_update exactly as §11.2, user records as user_message_chunk with messageId = record uuid (after stripLocalCommandMetadata), assistant records via content_chunk/tool_call/tool_result, TodoWrite → plan, skipping Agent/Task calls, synthetic login/usage-limit handling.
pub fn replay(records: &[Value], session_id: &str, cwd: &Path) -> Vec<(String, Value)> {
    let mut replay = Replay {
        root: session_id.to_string(),
        cwd,
        children: collect_children(records, session_id),
        updates: Vec::new(),
        tool_uses: HashMap::new(),
        tasks: TaskPlan::default(),
        failure_revisions: HashMap::new(),
    };
    let mut turn_id: Option<String> = None;
    // Claude answers messages from other sessions and finished background
    // tasks by itself; the prompt that wakes it is hidden, so a reply that
    // follows a finished turn begins a new one.
    let mut turn_over = false;
    // Pictures from before the last compaction would be most of a long
    // chat's bytes; their rows still say what they were.
    let pictures_from = records
        .iter()
        .rposition(|record| {
            truthy(record.get("isCompactSummary")) && parent_tool_use_id(record).is_none()
        })
        .unwrap_or(0);
    for (position, record) in records.iter().enumerate() {
        let parent = parent_tool_use_id(record);
        if truthy(record.get("isCompactSummary")) {
            let target = match parent {
                Some(parent) => replay.announce(parent, &mut Vec::new()),
                None => replay.root.clone(),
            };
            let mut update = json!({
                "sessionUpdate": "tool_call",
                "toolCallId": uuid_of(record),
                "title": "Compact conversation",
                "kind": "think",
                "status": "completed",
            });
            let at = str_field(record, "timestamp").and_then(crate::accounts::live::parse_rfc3339);
            super::super::stamp_replayed(&mut update, at, false);
            replay.updates.push((target, update));
            continue;
        }
        if str_field(record, "type") == Some("user") && parent.is_none() {
            if let Some(uuid) = str_field(record, "uuid").filter(|uuid| !uuid.is_empty()) {
                turn_id = Some(uuid.to_string());
            }
        }
        let Some(message) = record.get("message") else {
            continue;
        };
        let is_assistant = str_field(record, "type") == Some("assistant");
        if is_assistant && is_synthetic_login(message) {
            continue;
        }
        if is_assistant && parent.is_none() && is_synthetic_usage_limit(message) {
            if let Some(title) = assistant_text(message) {
                replay.usage_limit(record, turn_id.as_deref(), title);
            }
            continue;
        }
        let Some(mut content) = message.get("content").cloned() else {
            continue;
        };
        let target = match parent {
            Some(parent) => replay.announce(parent, &mut Vec::new()),
            None => replay.root.clone(),
        };
        let role = str_field(message, "role").unwrap_or_default();
        if role == "user" {
            match strip_local_command_metadata(&content) {
                Some(stripped) => content = stripped,
                None => continue,
            }
        }
        let message_id = message_id_for_grouping(record);
        let at = str_field(record, "timestamp").and_then(crate::accounts::live::parse_rfc3339);
        let updates = replay.convert(&content, role, message_id.as_deref());
        let main_thread = parent.is_none();
        let wakes = main_thread && wakes_agent(record);
        let woken_by = wakes
            .then(|| task_notification(record))
            .flatten()
            .and_then(|(agent_id, _)| {
                replay
                    .children
                    .iter()
                    .find(|child| child.agent_id.as_deref() == Some(agent_id.as_str()))
            })
            .map(|child| child.session_id.clone());
        let mut opens_turn =
            (is_assistant || wakes) && main_thread && turn_over && !updates.is_empty();
        for ended in ended_subagents(record) {
            replay.end(&ended);
        }
        for mut update in updates {
            if position < pictures_from {
                drop_pictures(&mut update);
            }
            // Claude's note to itself that background work ended wakes it; the turn
            // that follows is still one it began by itself.
            if main_thread
                && str_field(&update, "sessionUpdate") == Some("user_message_chunk")
                && !wakes
            {
                turn_over = false;
            }
            if let Some(session_id) = &woken_by {
                update["subagentSessionId"] = json!(session_id);
            }
            super::super::stamp_replayed(&mut update, at, opens_turn);
            opens_turn = false;
            replay.updates.push((target.clone(), update));
        }
        if is_assistant && main_thread {
            turn_over = str_field(message, "stop_reason") == Some("end_turn");
        }
    }
    replay.finish()
}

/// Drops the bytes of every picture in `value`, keeping what it was.
fn drop_pictures(value: &mut Value) {
    match value {
        Value::Array(items) => items.iter_mut().for_each(drop_pictures),
        Value::Object(fields) => {
            if fields.get("type").and_then(Value::as_str) == Some("image") {
                fields.remove("data");
                fields.remove("source");
            }
            fields.values_mut().for_each(drop_pictures);
        }
        _ => {}
    }
}

fn parent_tool_use_id(record: &Value) -> Option<&str> {
    str_field(record, "parent_tool_use_id")
}

fn message_id_for_grouping(record: &Value) -> Option<String> {
    let api_id = api_message_id(record).filter(|id| !id.is_empty());
    api_id
        .or_else(|| str_field(record, "uuid").filter(|uuid| !uuid.is_empty()))
        .map(str::to_string)
}

struct Child {
    tool_use_id: String,
    session_id: String,
    agent_id: Option<String>,
    parent_tool_use_id: Option<String>,
    name: String,
    task: String,
    reconstructable: bool,
    announced: bool,
    ended: bool,
    terminal_state: Option<Subagent>,
}

fn collect_children(records: &[Value], session_id: &str) -> Vec<Child> {
    let mut children: Vec<Child> = Vec::new();
    let mut terminal_states: Vec<(String, Subagent)> = Vec::new();
    let mut agent_ids: HashMap<String, String> = HashMap::new();
    let mut notified: Vec<(String, Subagent)> = Vec::new();
    for record in records {
        if let Some((agent_id, state)) = task_notification(record) {
            notified.push((agent_id, state));
            continue;
        }
        let Some(blocks) = record
            .get("message")
            .and_then(|message| message.get("content"))
            .and_then(Value::as_array)
        else {
            continue;
        };
        let owner = parent_tool_use_id(record).map(str::to_string);
        for block in blocks {
            let kind = str_field(block, "type").unwrap_or_default();
            if matches!(kind, "tool_result" | "mcp_tool_result") {
                if let Some(id) = str_field(block, "tool_use_id") {
                    let reported = record.get("toolUseResult");
                    if let Some(agent_id) =
                        reported.and_then(|reported| str_field(reported, "agentId"))
                    {
                        agent_ids.insert(agent_id.to_string(), id.to_string());
                    }
                    // A background agent's launch is not its end; its notification is.
                    if reported.and_then(|reported| str_field(reported, "status"))
                        == Some("async_launched")
                    {
                        continue;
                    }
                    let state = terminal_state(record, block);
                    match terminal_states.iter_mut().find(|(known, _)| known == id) {
                        Some(entry) => entry.1 = state,
                        None => terminal_states.push((id.to_string(), state)),
                    }
                }
            }
            if !matches!(kind, "tool_use" | "server_tool_use" | "mcp_tool_use") {
                continue;
            }
            let (Some(id), Some(name)) = (str_field(block, "id"), str_field(block, "name")) else {
                continue;
            };
            if !tools::is_subagent_tool(name) {
                continue;
            }
            let input = block.get("input").unwrap_or(&Value::Null);
            let field = |key: &str| str_field(input, key);
            let child = Child {
                tool_use_id: id.to_string(),
                session_id: format!("{session_id}:replay-subagent:{id}"),
                agent_id: None,
                parent_tool_use_id: owner.clone(),
                name: states::first_text(&[
                    field("name"),
                    field("description"),
                    field("subagent_type"),
                ])
                .unwrap_or_default(),
                task: states::first_text(&[field("prompt"), field("description")])
                    .unwrap_or_else(|| states::UNNAMED_TASK.to_owned()),
                reconstructable: true,
                announced: false,
                ended: false,
                terminal_state: terminal_states
                    .iter()
                    .find(|(known, _)| known == id)
                    .map(|(_, state)| *state),
            };
            match children.iter_mut().find(|known| known.tool_use_id == id) {
                Some(existing) => *existing = child,
                None => children.push(child),
            }
        }
    }
    for (id, state) in terminal_states {
        if let Some(child) = children.iter_mut().find(|child| child.tool_use_id == id) {
            child.terminal_state = Some(state);
        }
    }
    for (agent_id, state) in notified {
        let Some(spawner) = agent_ids.get(&agent_id) else {
            continue;
        };
        if let Some(child) = children
            .iter_mut()
            .find(|child| &child.tool_use_id == spawner)
        {
            child.terminal_state = Some(state);
        }
    }
    for child in children.iter_mut() {
        child.agent_id = agent_ids
            .iter()
            .find(|(_, spawner)| **spawner == child.tool_use_id)
            .map(|(agent_id, _)| agent_id.clone());
        if child.name.is_empty() {
            child.name = states::unnamed(child.agent_id.as_deref().unwrap_or(&child.tool_use_id));
        }
    }
    children
}

/// The subagents `record` says have ended: a call's answer, unless it only says
/// the subagent went to the background, or the notice a background one sends.
fn ended_subagents(record: &Value) -> Vec<Ended> {
    if let Some((agent_id, _)) = task_notification(record) {
        return vec![Ended::Agent(agent_id)];
    }
    let reported = record.get("toolUseResult");
    if reported.and_then(|reported| str_field(reported, "status")) == Some("async_launched") {
        return Vec::new();
    }
    record
        .pointer("/message/content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|block| {
            matches!(
                str_field(block, "type"),
                Some("tool_result" | "mcp_tool_result")
            )
        })
        .filter_map(|block| str_field(block, "tool_use_id"))
        .map(|id| Ended::Call(id.to_owned()))
        .collect()
}

enum Ended {
    Call(String),
    Agent(String),
}

fn wakes_agent(record: &Value) -> bool {
    json::joined_text(record.pointer("/message/content")).contains("<task-notification>")
}

/// The agent and end state a `<task-notification>` Claude Code wrote to itself reports.
fn task_notification(record: &Value) -> Option<(String, Subagent)> {
    if str_field(record, "type") != Some("user") {
        return None;
    }
    let said = json::joined_text(record.pointer("/message/content"));
    let tag = |name: &str| {
        said.split(&format!("<{name}>"))
            .nth(1)?
            .split(&format!("</{name}>"))
            .next()
            .map(str::trim)
    };
    if !said.contains("<task-notification>") {
        return None;
    }
    let state = states::outcome(tag("status")?).map(Subagent::from)?;
    Some((tag("task-id")?.to_string(), state))
}

/// How a subagent whose call answered with `result` ended. Live, Claude Code
/// says so in a status; the transcript keeps only the answer, which says the
/// person stopped it in the words it stops any call with.
fn terminal_state(record: &Value, result: &Value) -> Subagent {
    if result.get("is_error") != Some(&Value::Bool(true)) {
        return Subagent::Completed;
    }
    // A call stopped mid-run keeps the refusal text, but the record says the person stopped it.
    if str_field(record, "toolUseResult") == Some("User rejected tool use") {
        return Subagent::Cancelled;
    }
    let said = json::joined_text(result.get("content"));
    if INTERRUPTION_PREFIXES
        .iter()
        .any(|prefix| said.trim_start().starts_with(prefix))
    {
        Subagent::Cancelled
    } else {
        Subagent::Failed
    }
}

struct Replay<'a> {
    root: String,
    cwd: &'a Path,
    children: Vec<Child>,
    updates: Vec<(String, Value)>,
    tool_uses: HashMap<String, (String, Value)>,
    tasks: TaskPlan,
    failure_revisions: HashMap<String, u64>,
}

impl Replay<'_> {
    fn child_index(&self, tool_use_id: &str) -> Option<usize> {
        self.children
            .iter()
            .position(|child| child.tool_use_id == tool_use_id)
    }

    fn session_of(&self, tool_use_id: Option<&str>) -> String {
        tool_use_id.and_then(|id| self.child_index(id)).map_or_else(
            || self.root.clone(),
            |i| self.children[i].session_id.clone(),
        )
    }

    fn announce(&mut self, tool_use_id: &str, ancestors: &mut Vec<String>) -> String {
        let index = match self.child_index(tool_use_id) {
            Some(index) => index,
            None => {
                self.children.push(Child {
                    tool_use_id: tool_use_id.to_string(),
                    session_id: format!("{}:replay-subagent:{tool_use_id}", self.root),
                    agent_id: None,
                    parent_tool_use_id: None,
                    name: states::unnamed(tool_use_id),
                    task: states::UNNAMED_TASK.to_owned(),
                    reconstructable: false,
                    announced: false,
                    ended: false,
                    terminal_state: None,
                });
                self.children.len() - 1
            }
        };
        if ancestors.iter().any(|ancestor| ancestor == tool_use_id) {
            let child = &mut self.children[index];
            child.reconstructable = false;
            child.terminal_state = None;
            child.parent_tool_use_id = None;
        }
        if let Some(parent) = self.children[index].parent_tool_use_id.clone() {
            ancestors.push(tool_use_id.to_string());
            self.announce(&parent, ancestors);
            ancestors.pop();
        }
        if !self.children[index].announced {
            let parent_session =
                self.session_of(self.children[index].parent_tool_use_id.as_deref());
            let child = &self.children[index];
            let update = states::spawned(&child.session_id, &child.name, &child.task);
            self.updates.push((parent_session, update));
            self.children[index].announced = true;
        }
        self.children[index].session_id.clone()
    }

    /// Says how a subagent ended where it ended, so its card is settled
    /// beside the work around it rather than with the chat's last turn.
    fn end(&mut self, ended: &Ended) {
        let index = self.children.iter().position(|child| match ended {
            Ended::Call(id) => child.tool_use_id == *id,
            Ended::Agent(id) => child.agent_id.as_deref() == Some(id.as_str()),
        });
        let Some(index) = index else {
            return;
        };
        let child = &self.children[index];
        if !child.announced || child.ended || !child.reconstructable {
            return;
        }
        let Some(state) = child.terminal_state else {
            return;
        };
        let update = state.update(&child.session_id);
        let parent_session = self.session_of(child.parent_tool_use_id.as_deref());
        self.children[index].ended = true;
        self.updates.push((parent_session, update));
    }

    fn usage_limit(&mut self, record: &Value, turn_id: Option<&str>, title: String) {
        let id = match turn_id {
            Some(turn) => format!("{turn}:error"),
            None => format!("{}:history-error:{}", self.root, js_str(record.get("uuid"))),
        };
        let revision = self.failure_revisions.entry(id.clone()).or_insert(0);
        *revision += 1;
        let update = json!({
            "sessionUpdate": "session_info_update",
            "_meta": { "jetbrains": { "air": { "version": 1, "sessionFailure": {
                "id": id,
                "revision": *revision,
                "category": "limit",
                "severity": "error",
                "title": title,
                "actions": [],
            } } } },
        });
        self.updates.push((self.root.clone(), update));
    }

    fn convert(&mut self, content: &Value, role: &str, message_id: Option<&str>) -> Vec<Value> {
        let blocks = match content {
            Value::String(_) => {
                return tools::content_chunk(content, role, message_id)
                    .into_iter()
                    .collect()
            }
            Value::Array(blocks) => blocks,
            _ => return Vec::new(),
        };
        let mut updates = Vec::new();
        for block in blocks {
            let kind = str_field(block, "type").unwrap_or_default();
            match kind {
                "text" | "text_delta" | "image" | "thinking" | "thinking_delta" => {
                    updates.extend(tools::content_chunk(block, role, message_id));
                }
                "tool_use" | "server_tool_use" | "mcp_tool_use" => {
                    updates.extend(self.tool_use(block));
                }
                "tool_result"
                | "tool_search_tool_result"
                | "web_fetch_tool_result"
                | "web_search_tool_result"
                | "code_execution_tool_result"
                | "bash_code_execution_tool_result"
                | "text_editor_code_execution_tool_result"
                | "mcp_tool_result" => updates.extend(self.tool_result(block)),
                _ => {}
            }
        }
        updates
    }

    fn tool_use(&mut self, block: &Value) -> Option<Value> {
        let id = js_str(block.get("id"));
        let name = js_str(block.get("name"));
        let input = block.get("input").cloned().unwrap_or(Value::Null);
        let seen = self
            .tool_uses
            .insert(id.clone(), (name.clone(), input.clone()))
            .is_some();
        if name == "TodoWrite" {
            return tools::todo_plan(&input);
        }
        if tools::is_plan_tool(&name) || tools::is_subagent_tool(&name) {
            return None;
        }
        let mut update = if seen {
            tools::tool_call_refined(&id, &name, &input, self.cwd)
        } else {
            tools::tool_call(&id, &name, &input, self.cwd)
        };
        if block.get("input").is_none() {
            if let Some(map) = update.as_object_mut() {
                map.remove("rawInput");
            }
        }
        Some(update)
    }

    fn tool_result(&mut self, block: &Value) -> Option<Value> {
        let id = js_str(block.get("tool_use_id"));
        let (name, input) = self.tool_uses.remove(&id)?;
        if tools::is_plan_tool(&name) {
            return if name == "TodoWrite" {
                None
            } else {
                self.tasks.apply_result(&name, &input, block, None)
            };
        }
        if tools::is_subagent_tool(&name) {
            return None;
        }
        Some(tools::tool_result(
            &id, &name, &input, block, None, self.cwd,
        ))
    }

    fn finish(mut self) -> Vec<(String, Value)> {
        // A call with no result is one the agent's last process died in, and nothing will finish it now.
        let mut open: Vec<(String, String)> = self
            .tool_uses
            .keys()
            .filter_map(|id| {
                self.updates
                    .iter()
                    .find(|(_, update)| {
                        str_field(update, "sessionUpdate") == Some("tool_call")
                            && str_field(update, "toolCallId") == Some(id.as_str())
                    })
                    .map(|(session, _)| (session.clone(), id.clone()))
            })
            .collect();
        open.sort();
        for (session, id) in open {
            let update = json!({ "sessionUpdate": "tool_call_update", "toolCallId": id, "status": "cancelled" });
            self.updates.push((session, update));
        }
        for index in (0..self.children.len()).rev() {
            let child = &self.children[index];
            if !child.announced || child.ended {
                continue;
            }
            let state = match child.terminal_state {
                Some(state) if child.reconstructable => state,
                _ => Subagent::Disconnected,
            };
            let update = state.update(&child.session_id);
            let parent_session = self.session_of(child.parent_tool_use_id.as_deref());
            self.updates.push((parent_session, update));
        }
        self.updates
    }
}

fn is_synthetic_login(message: &Value) -> bool {
    if !failure::is_synthetic(message) {
        return false;
    }
    match message
        .get("content")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
    {
        Some([block]) => {
            str_field(block, "type") == Some("text")
                && str_field(block, "text").is_some_and(failure::asks_sign_in)
        }
        _ => false,
    }
}

fn is_synthetic_usage_limit(message: &Value) -> bool {
    failure::is_synthetic(message)
        && assistant_text(message).is_some_and(|text| failure::out_of_usage(&text))
}

fn assistant_text(message: &Value) -> Option<String> {
    let text = match message.get("content")? {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|block| str_field(block, "type") == Some("text"))
            .filter_map(|block| str_field(block, "text"))
            .collect(),
        _ => return None,
    };
    (!text.is_empty()).then_some(text)
}

const TRANSCRIPT_MARKERS: [&str; 6] = [
    "command-name",
    "command-message",
    "command-args",
    "local-command-stdout",
    "local-command-stderr",
    "system-reminder",
];

const REPLAY_HIDDEN_COMMANDS: [&str; 7] = [
    "/context",
    "/heapdump",
    "/extra-usage",
    "/compact",
    "/model",
    "/status",
    "/usage",
];

/// User content with local-command and injected-context marker tags removed, or None when
/// nothing a person typed remains.
pub fn strip_local_command_metadata(content: &Value) -> Option<Value> {
    match content {
        Value::String(text) => strip_command_text(text).map(Value::String),
        Value::Array(blocks) => {
            let mut kept = Vec::new();
            for block in blocks {
                match block.get("text").and_then(Value::as_str) {
                    Some(text) if str_field(block, "type") == Some("text") => {
                        if let Some(stripped) = strip_command_text(text) {
                            let mut block = block.clone();
                            block["text"] = Value::String(stripped);
                            kept.push(block);
                        }
                    }
                    _ => kept.push(block.clone()),
                }
            }
            (!kept.is_empty()).then_some(Value::Array(kept))
        }
        other => Some(other.clone()),
    }
}

fn strip_command_text(text: &str) -> Option<String> {
    let stripped = strip_marker_tags(text);
    if !stripped.trim_matches(is_js_whitespace).is_empty() {
        return Some(stripped);
    }
    command_invocation(text)
}

fn strip_marker_tags(text: &str) -> String {
    let markers: Vec<(String, String)> = TRANSCRIPT_MARKERS
        .iter()
        .map(|tag| (format!("<{tag}>"), format!("</{tag}>")))
        .collect();
    let mut dead = vec![false; markers.len()];
    let mut result = String::new();
    let mut copied_up_to = 0;
    let mut index = 0;
    let bytes = text.as_bytes();
    while index < bytes.len() {
        if bytes[index] == b'<' {
            let found = markers
                .iter()
                .enumerate()
                .find(|(i, (open, _))| !dead[*i] && bytes[index..].starts_with(open.as_bytes()));
            if let Some((marker, (open, close))) = found {
                match text[index + open.len()..].find(close.as_str()) {
                    Some(offset) => {
                        result.push_str(&text[copied_up_to..index]);
                        index += open.len() + offset + close.len();
                        copied_up_to = index;
                        continue;
                    }
                    None => dead[marker] = true,
                }
            }
        }
        index += 1;
    }
    result.push_str(&text[copied_up_to..]);
    result
}

fn marker_text<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(&close)? + start;
    Some(&text[start..end])
}

fn command_invocation(text: &str) -> Option<String> {
    if marker_text(text, "local-command-stdout").is_some()
        || marker_text(text, "local-command-stderr").is_some()
    {
        return None;
    }
    let name = marker_text(text, "command-name")?.trim_matches(is_js_whitespace);
    if !name.starts_with('/') {
        return None;
    }
    let command = name.split(' ').next().unwrap_or(name);
    if REPLAY_HIDDEN_COMMANDS.contains(&command) {
        return None;
    }
    match marker_text(text, "command-args").map(|args| args.trim_matches(is_js_whitespace)) {
        Some(args) if !args.is_empty() => Some(format!("{name} {args}")),
        _ => Some(name.to_string()),
    }
}

#[cfg(test)]
mod matches_adapter;
#[cfg(test)]
mod matches_sdk;
#[cfg(test)]
mod tests;
