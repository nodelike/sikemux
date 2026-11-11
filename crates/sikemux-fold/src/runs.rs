//! A message's runs of tool calls, each counted on its first call: what the run did as
//! a client words it in one sentence, and the lines it opens onto.

use std::collections::HashSet;

use sikemux_view::chat::{
    ChatPart, RunClause, RunLine, ToolCall, ToolFamily, ToolIntent, ToolRun, ToolStatus,
};

/// The most names a clause keeps.
const MAX_NAMES: usize = 3;

/// Whether a part goes in a run: a call that shows as a row, not as a page or a GIF.
pub(super) fn in_run(part: &ChatPart) -> bool {
    matches!(
        part,
        ChatPart::Tool {
            page: None,
            gif: None,
            ..
        }
    )
}

/// The parts whose run changed, after working out every run of `parts` again.
pub(super) fn restate(parts: &mut [ChatPart]) -> Vec<usize> {
    let mut changed = Vec::new();
    let mut at = 0;
    while at < parts.len() {
        if !in_run(&parts[at]) {
            if let ChatPart::Tool { run, .. } = &mut parts[at] {
                if run.take().is_some() {
                    changed.push(at);
                }
            }
            at += 1;
            continue;
        }
        let end = (at..parts.len())
            .find(|&index| !in_run(&parts[index]))
            .unwrap_or(parts.len());
        let counted = count(&parts[at..end]);
        for (index, part) in parts.iter_mut().enumerate().take(end).skip(at) {
            let ChatPart::Tool { run, .. } = part else {
                continue;
            };
            let next = (index == at).then(|| Box::new(counted.clone()));
            if *run != next {
                *run = next;
                changed.push(index);
            }
        }
        at = end;
    }
    changed
}

/// The family a call is counted under: a command that only looks counts with the file tools.
fn counted_family(tool: &ToolCall) -> ToolFamily {
    let row = &tool.row;
    let looks = matches!(
        row.intent,
        ToolIntent::Read | ToolIntent::View | ToolIntent::Search | ToolIntent::List
    );
    match row.family {
        ToolFamily::Shell if looks => ToolFamily::Files,
        family => family,
    }
}

struct Call<'a> {
    tool: &'a ToolCall,
    failed: bool,
    took: Option<u64>,
}

fn calls(parts: &[ChatPart]) -> Vec<Call<'_>> {
    parts
        .iter()
        .filter_map(|part| match part {
            ChatPart::Tool {
                tool,
                failure,
                started_at,
                ended_at,
                ..
            } => Some(Call {
                tool,
                failed: tool.status == Some(ToolStatus::Failed) || failure.is_some(),
                took: ended_at
                    .zip(*started_at)
                    .map(|(ended, started)| ended.saturating_sub(started)),
            }),
            _ => None,
        })
        .collect()
}

pub(super) fn count(parts: &[ChatPart]) -> ToolRun {
    let calls = calls(parts);
    let (hidden, shown): (Vec<&Call>, Vec<&Call>) = calls
        .iter()
        .partition(|call| call.tool.row.family == ToolFamily::Housekeeping);
    let mut said: Vec<(RunClause, HashSet<String>, bool)> = Vec::new();
    for call in &shown {
        let row = &call.tool.row;
        let family = counted_family(call.tool);
        let at = match said
            .iter()
            .position(|(clause, ..)| clause.intent == row.intent && clause.family == family)
        {
            Some(at) => at,
            None => {
                let clause = RunClause {
                    intent: row.intent,
                    family,
                    count: 0,
                    names: Vec::new(),
                    took: None,
                };
                said.push((clause, HashSet::new(), true));
                said.len() - 1
            }
        };
        let (clause, things, all_named) = &mut said[at];
        clause.count += row.calls();
        match thing(call.tool) {
            Some(thing) => {
                things.insert(thing);
            }
            None => *all_named = false,
        }
        if let Some(name) = name(call.tool) {
            if !clause.names.contains(&name) && clause.names.len() < MAX_NAMES {
                clause.names.push(name);
            }
        }
        if let Some(took) = call.took {
            clause.took = Some(clause.took.unwrap_or_default() + took);
        }
    }
    let said: Vec<RunClause> = said
        .into_iter()
        .map(|(mut clause, things, all_named)| {
            if all_named && counts_things(clause.intent) {
                clause.count = things.len() as u64;
            }
            clause
        })
        .collect();
    let looks_only = said.iter().all(|clause| {
        matches!(
            clause.intent,
            ToolIntent::Read | ToolIntent::View | ToolIntent::Search | ToolIntent::List
        ) && clause.family == ToolFamily::Files
    });
    ToolRun {
        explored: said.len() > 1 && looks_only,
        failed: shown.iter().filter(|call| call.failed).count() as u64,
        hidden: hidden.iter().map(|call| call.tool.row.calls()).sum(),
        lines: lines(&shown),
        said,
    }
}

/// Failed calls first, each on its own line; then the rest in order, neighbours that did
/// the same thing together.
fn lines(shown: &[&Call]) -> Vec<RunLine> {
    let line = |call: &Call| RunLine {
        intent: call.tool.row.intent,
        family: counted_family(call.tool),
        calls: vec![call.tool.tool_call_id.clone()],
        failed: call.failed,
    };
    let mut lines: Vec<RunLine> = shown
        .iter()
        .filter(|call| call.failed)
        .map(|call| line(call))
        .collect();
    let failed = lines.len();
    for call in shown.iter().filter(|call| !call.failed) {
        match lines[failed..].last_mut() {
            Some(last)
                if last.intent == call.tool.row.intent
                    && last.family == counted_family(call.tool)
                    && merges(call.tool.row.intent) =>
            {
                last.calls.push(call.tool.tool_call_id.clone())
            }
            _ => lines.push(line(call)),
        }
    }
    lines
}

/// Whether neighbours doing this read as one line. Changes and commands keep a line each,
/// since each has its own diff or output to open.
fn merges(intent: ToolIntent) -> bool {
    !matches!(
        intent,
        ToolIntent::Edit
            | ToolIntent::Write
            | ToolIntent::Run
            | ToolIntent::Test
            | ToolIntent::Build
            | ToolIntent::Git
            | ToolIntent::Commit
            | ToolIntent::Message
    )
}

/// Whether a clause counts what it touched, such as files read, rather than its calls.
fn counts_things(intent: ToolIntent) -> bool {
    matches!(
        intent,
        ToolIntent::Read
            | ToolIntent::View
            | ToolIntent::Edit
            | ToolIntent::Write
            | ToolIntent::Move
            | ToolIntent::Delete
            | ToolIntent::List
    )
}

/// What a call touched, by which two calls touching the same thing count once.
fn thing(tool: &ToolCall) -> Option<String> {
    let row = &tool.row;
    let path = row.path.as_deref().map(|path| {
        path.rsplit_once(':')
            .filter(|(_, line)| line.bytes().all(|b| b.is_ascii_digit() || b == b'-'))
            .map_or(path, |(file, _)| file)
    });
    path.map(str::to_owned)
        .or_else(|| row.subject.clone())
        .or_else(|| (!row.target.is_empty()).then(|| row.target.clone()))
}

/// Who or what a clause names: the agent written to or waited on, the service asked.
fn name(tool: &ToolCall) -> Option<String> {
    let row = &tool.row;
    match row.family {
        ToolFamily::Agents | ToolFamily::Waiting => row.subject.clone(),
        ToolFamily::Git | ToolFamily::Ops | ToolFamily::Trackers | ToolFamily::Other => row
            .plugin
            .as_deref()
            .map(|plugin| plugin.strip_prefix("sikemux.").unwrap_or(plugin).to_owned())
            .or_else(|| row.server.clone()),
        _ => None,
    }
}
