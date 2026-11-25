//! A message read as it is written: each new piece of text re-reads only the
//! blocks at its end, and says what changed as a patch to the tree a client
//! already holds. Markup still open at the end is drawn as if closed until the
//! message is done.

use serde::Serialize;
use serde_json::Value;

use crate::{fences, mend::mend, parse, parse_blocks, MarkdownOptions, MdElement};

/// The line being written may still join the block before it, as a table row
/// or a heading's underline does, so the last two blocks stay open.
const OPEN_BLOCKS: usize = 2;

/// One change to a tree a client holds. `p` is a path of indexes from the
/// top-level blocks down through each element's children (`c`, or a table's
/// `rows`, its cells and their contents). Positions in text count UTF-16
/// units, as clients slice it.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum MdOp {
    /// The list at `p` keeps its first `at` entries and gains `c`.
    List {
        p: Vec<usize>,
        at: usize,
        c: Vec<Value>,
    },
    /// The text at `p`, or a code block's `v` there, keeps its first `at` units and gains `s`.
    Text { p: Vec<usize>, at: usize, s: String },
}

/// The changes that turn one tree into the next, in order.
pub type Patch = Vec<MdOp>;

/// What the core sends with a piece of an answer: the answer's length once it is added, and the
/// changes that bring the tree up to it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MdChange {
    pub n: usize,
    pub o: Patch,
}

fn units(text: &str) -> usize {
    if text.is_ascii() {
        text.len()
    } else {
        text.encode_utf16().count()
    }
}

/// The key an element keeps what grows inside it under.
const GROWS: [&str; 3] = ["c", "rows", "v"];

fn shared_text(old: &str, new: &str) -> usize {
    let diff = old
        .as_bytes()
        .iter()
        .zip(new.as_bytes())
        .position(|(a, b)| a != b)
        .unwrap_or_else(|| old.len().min(new.len()));
    old.floor_char_boundary(diff)
}

fn text_op(path: Vec<usize>, old: &str, new: &str) -> Vec<MdOp> {
    if old == new {
        return Vec::new();
    }
    let kept = shared_text(old, new);
    vec![MdOp::Text {
        p: path,
        at: units(old.get(..kept).unwrap_or_default()),
        s: new.get(kept..).unwrap_or_default().to_owned(),
    }]
}

/// The changes inside `old` that make it `new`, when `new` is the same node grown.
fn grown(path: Vec<usize>, old: &Value, new: &Value) -> Option<Vec<MdOp>> {
    match (old, new) {
        (Value::String(a), Value::String(b)) => Some(text_op(path, a, b)),
        (Value::Array(a), Value::Array(b)) => Some(list_ops(path, a, b, 0)),
        (Value::Object(a), Value::Object(b)) => {
            let key = GROWS.into_iter().find(|key| a.contains_key(*key))?;
            let same_shell = a.len() == b.len()
                && a.iter()
                    .all(|(name, value)| name == key || b.get(name) == Some(value));
            if !same_shell {
                return None;
            }
            match (a.get(key)?, b.get(key)?) {
                (Value::String(a), Value::String(b)) => Some(text_op(path, a, b)),
                (Value::Array(a), Value::Array(b)) => Some(list_ops(path, a, b, 0)),
                _ => None,
            }
        }
        _ => None,
    }
}

fn list_ops(path: Vec<usize>, old: &[Value], new: &[Value], first: usize) -> Vec<MdOp> {
    let same = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    if same == old.len() && same == new.len() {
        return Vec::new();
    }
    let mut at = path.clone();
    at.push(first + same);
    let inside = old
        .get(same)
        .zip(new.get(same))
        .and_then(|(a, b)| grown(at, a, b));
    let (mut ops, from) = match inside {
        Some(ops) => (ops, same + 1),
        None => (Vec::new(), same),
    };
    if from < old.len().max(new.len()) {
        ops.push(MdOp::List {
            p: path,
            at: first + from,
            c: new.get(from..).unwrap_or_default().to_vec(),
        });
    }
    ops
}

/// The changes that turn the top-level blocks `old` into `new`, whose indexes start at `first`.
pub fn diff(old: &[Value], new: &[Value], first: usize) -> Patch {
    list_ops(Vec::new(), old, new, first)
}

fn values(blocks: Vec<MdElement>) -> Vec<Value> {
    blocks
        .into_iter()
        .filter_map(|block| serde_json::to_value(block).ok())
        .collect()
}

#[derive(Clone, Copy)]
struct Fence {
    mark: u8,
    width: usize,
    indent: usize,
    quote_depth: usize,
    in_list: bool,
    open_at: usize,
}

struct Scanner {
    line_start: usize,
    fence: Option<Fence>,
    list_indent: Option<usize>,
    blank_before: bool,
    in_indented_code: bool,
    pending_label: bool,
}

impl Scanner {
    fn new() -> Self {
        Self {
            line_start: 0,
            fence: None,
            list_indent: None,
            blank_before: true,
            in_indented_code: false,
            pending_label: false,
        }
    }

    fn advance(&mut self, text: &str, prev_len: usize, gfm: bool) -> bool {
        let mut reaches = false;
        while let Some(rel) = text.get(self.line_start..).and_then(|rest| rest.find('\n')) {
            let line_end = self.line_start + rel + 1;
            let line = text.get(self.line_start..line_end).unwrap_or_default();
            if self.commit_line(line, self.line_start, gfm) {
                reaches = true;
            }
            self.line_start = line_end;
        }
        if reaches {
            return true;
        }
        let tail = text.get(self.line_start..).unwrap_or_default();
        if tail.is_empty() || self.fence.is_some() {
            return false;
        }
        let overlap = prev_len.max(self.line_start).saturating_sub(1);
        let added = text.get(overlap..).unwrap_or(tail);
        if !added.contains("]:") && !(gfm && added.contains("[^")) {
            return false;
        }
        self.inspect_tail(tail, gfm)
    }

    fn open_top_fence(&self, text: &str) -> Option<Fence> {
        let fence = self.fence?;
        if fence.quote_depth != 0 || fence.in_list || fence.indent != 0 {
            return None;
        }
        let tail = text.get(self.line_start..).unwrap_or_default();
        if !tail.is_empty() && closes_top_fence(tail, fence.mark, fence.width) {
            return None;
        }
        Some(fence)
    }

    fn commit_line(&mut self, raw_line: &str, line_at: usize, gfm: bool) -> bool {
        let line = raw_line.strip_suffix('\n').unwrap_or(raw_line);
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(fence) = self.fence {
            if fence.quote_depth == 0 {
                let closed = if fence.in_list {
                    fences::closes(line, fence.mark, fence.width)
                } else {
                    closes_top_fence(line, fence.mark, fence.width)
                };
                if closed {
                    self.fence = None;
                    self.blank_before = false;
                    self.in_indented_code = false;
                    self.pending_label = false;
                }
                return false;
            }
            if let Some(rest) = strip_quotes(line, fence.quote_depth) {
                if closes_top_fence(rest, fence.mark, fence.width) {
                    self.fence = None;
                    self.blank_before = false;
                    self.in_indented_code = false;
                    self.pending_label = false;
                }
                return false;
            }
            self.fence = None;
        }

        if line.trim().is_empty() {
            self.blank_before = true;
            self.pending_label = false;
            return false;
        }

        let (quote_depth, quoted) = strip_all_quotes(line);
        if quote_depth > 0 {
            self.in_indented_code = false;
            self.list_indent = None;
            let indent = leading_spaces(quoted);
            let rest = quoted.get(indent..).unwrap_or_default();
            if indent <= 3 {
                if let Some((mark, width)) = fences::fence_of(rest) {
                    self.fence = Some(Fence {
                        mark,
                        width,
                        indent,
                        quote_depth,
                        in_list: false,
                        open_at: line_at,
                    });
                    self.blank_before = false;
                    self.pending_label = false;
                    return false;
                }
            }
            let (reaches, next_pending) =
                line_reaches_back(rest, indent <= 3, self.pending_label, gfm);
            self.pending_label = next_pending;
            self.blank_before = false;
            return reaches;
        }

        let indent = fences::indent_of(line);
        let rest = line.trim_start_matches([' ', '\t']);
        if self.list_indent.is_some_and(|content| indent < content) {
            self.list_indent = None;
        }
        let base_indent = self.list_indent.unwrap_or(0);
        if indent >= base_indent + 4 && (self.blank_before || self.in_indented_code) {
            self.in_indented_code = true;
            self.blank_before = false;
            self.pending_label = false;
            return false;
        }
        self.in_indented_code = false;

        if indent <= base_indent + 3 {
            if let Some(marker) = fences::list_marker(rest) {
                self.list_indent = Some(indent + marker);
                let after = rest.get(marker..).unwrap_or_default();
                if let Some((mark, width)) = fences::fence_of(after) {
                    self.fence = Some(Fence {
                        mark,
                        width,
                        indent: 0,
                        quote_depth: 0,
                        in_list: true,
                        open_at: line_at,
                    });
                    self.blank_before = false;
                    self.pending_label = false;
                    return false;
                }
                let (reaches, next_pending) = line_reaches_back(after, true, false, gfm);
                self.pending_label = next_pending;
                self.blank_before = false;
                return reaches;
            }
            if let Some((mark, width)) = fences::fence_of(rest) {
                let in_list = self.list_indent.is_some();
                self.fence = Some(Fence {
                    mark,
                    width,
                    indent: if in_list { 0 } else { indent },
                    quote_depth: 0,
                    in_list,
                    open_at: line_at,
                });
                self.blank_before = false;
                self.pending_label = false;
                return false;
            }
        }

        let (reaches, next_pending) =
            line_reaches_back(rest, indent <= base_indent + 3, self.pending_label, gfm);
        self.pending_label = next_pending;
        self.blank_before = false;
        reaches
    }

    fn inspect_tail(&self, raw_line: &str, gfm: bool) -> bool {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if line.trim().is_empty() {
            return false;
        }
        let (quote_depth, quoted) = strip_all_quotes(line);
        if quote_depth > 0 {
            let indent = leading_spaces(quoted);
            let rest = quoted.get(indent..).unwrap_or_default();
            if indent <= 3 && fences::fence_of(rest).is_some() {
                return false;
            }
            return line_reaches_back(rest, indent <= 3, self.pending_label, gfm).0;
        }
        let indent = fences::indent_of(line);
        let rest = line.trim_start_matches([' ', '\t']);
        let base_indent = self
            .list_indent
            .filter(|&content| indent >= content)
            .unwrap_or(0);
        if indent >= base_indent + 4 && (self.blank_before || self.in_indented_code) {
            return false;
        }
        if indent <= base_indent + 3 {
            if let Some(marker) = fences::list_marker(rest) {
                let after = rest.get(marker..).unwrap_or_default();
                if fences::fence_of(after).is_some() {
                    return false;
                }
                return line_reaches_back(after, true, false, gfm).0;
            }
            if fences::fence_of(rest).is_some() {
                return false;
            }
        }
        line_reaches_back(rest, indent <= base_indent + 3, self.pending_label, gfm).0
    }
}

fn leading_spaces(s: &str) -> usize {
    s.bytes().take_while(|&b| b == b' ').count()
}

fn strip_quotes(mut line: &str, count: usize) -> Option<&str> {
    for _ in 0..count {
        let spaces = leading_spaces(line);
        if spaces > 3 {
            return None;
        }
        let rest = line.get(spaces..)?.strip_prefix('>')?;
        line = rest.strip_prefix(' ').unwrap_or(rest);
    }
    Some(line)
}

fn strip_all_quotes(mut line: &str) -> (usize, &str) {
    let mut depth = 0;
    while let Some(next) = strip_quotes(line, 1) {
        depth += 1;
        line = next;
    }
    (depth, line)
}

fn closes_top_fence(line: &str, mark: u8, width: usize) -> bool {
    let bytes = line.as_bytes();
    let spaces = bytes.iter().take_while(|&&b| b == b' ').count();
    if spaces > 3 {
        return false;
    }
    let Some(rest) = bytes.get(spaces..) else {
        return false;
    };
    let run = rest.iter().take_while(|&&b| b == mark).count();
    if run < width {
        return false;
    }
    rest.get(run..).is_some_and(|after| {
        after
            .iter()
            .all(|&b| matches!(b, b' ' | b'\t' | b'\r' | b'\n'))
    })
}

fn line_cannot_close(line: &str, mark: u8) -> bool {
    let bytes = line.as_bytes();
    let spaces = bytes.iter().take_while(|&&b| b == b' ').count();
    if spaces > 3 {
        return true;
    }
    let Some(rest) = bytes.get(spaces..) else {
        return false;
    };
    let run = rest.iter().take_while(|&&b| b == mark).count();
    let after = rest.get(run..).unwrap_or_default();
    if run == 0 {
        !after.is_empty()
    } else {
        !after.iter().all(|&b| matches!(b, b' ' | b'\t'))
    }
}

fn scan_label_close(bytes: &[u8]) -> Option<Option<usize>> {
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        match b {
            b'\\' => i += 2,
            b'[' => return None,
            b']' => return Some(Some(i)),
            _ => i += 1,
        }
    }
    Some(None)
}

fn has_footnote_outside_code(line: &str) -> bool {
    let bytes = line.as_bytes();
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        if b == b'\\' {
            i += 2;
            continue;
        }
        if b == b'`' {
            let run = bytes
                .get(i..)
                .map_or(1, |rest| rest.iter().take_while(|&&c| c == b'`').count());
            let mut j = i + run;
            let mut closed = None;
            while let Some(&c) = bytes.get(j) {
                if c == b'`' {
                    let close_run = bytes
                        .get(j..)
                        .map_or(1, |rest| rest.iter().take_while(|&&x| x == b'`').count());
                    if close_run == run {
                        closed = Some(j + close_run);
                        break;
                    }
                    j += close_run;
                } else {
                    j += 1;
                }
            }
            i = closed.unwrap_or(i + run);
            continue;
        }
        if b == b'[' && bytes.get(i + 1) == Some(&b'^') {
            return true;
        }
        i += 1;
    }
    false
}

fn line_reaches_back(
    rest: &str,
    allow_link_ref: bool,
    pending_label: bool,
    gfm: bool,
) -> (bool, bool) {
    let mut next_pending = false;
    if allow_link_ref {
        if let Some(after_open) = rest.strip_prefix('[') {
            match scan_label_close(after_open.as_bytes()) {
                Some(Some(close)) => {
                    let label = after_open.get(..close).unwrap_or_default();
                    if !label.trim().is_empty()
                        && after_open.as_bytes().get(close + 1) == Some(&b':')
                    {
                        return (true, false);
                    }
                }
                Some(None) => next_pending = true,
                None => {}
            }
        } else if pending_label {
            match scan_label_close(rest.as_bytes()) {
                Some(Some(close)) => {
                    if rest.as_bytes().get(close + 1) == Some(&b':') {
                        return (true, false);
                    }
                }
                Some(None) => next_pending = true,
                None => {}
            }
        }
    }
    if gfm && rest.contains("[^") && has_footnote_outside_code(rest) {
        return (true, next_pending);
    }
    (false, next_pending)
}

struct FastPre {
    mark: u8,
    width: usize,
    synced_len: usize,
    line_start: usize,
    line_disqualified: bool,
    body_utf16: usize,
    synthetic_nl: bool,
}

/// One message as it grows, and the blocks at its end its clients were last sent.
pub struct Doc {
    options: MarkdownOptions,
    text: String,
    units: usize,
    /// Where the open blocks start in `text`.
    base: usize,
    /// How many blocks before `base` no longer change.
    settled: usize,
    /// The blocks from `base` on, as last sent.
    open: Vec<Value>,
    whole: bool,
    /// Whether the last block sent had markup closed that the text leaves open.
    mended: bool,
    scan: Scanner,
    fast_pre: Option<FastPre>,
}

impl Doc {
    /// A message whose clients hold nothing of its tree yet.
    pub fn new(options: MarkdownOptions) -> Self {
        Self {
            options,
            text: String::new(),
            units: 0,
            base: 0,
            settled: 0,
            open: Vec::new(),
            whole: false,
            mended: false,
            scan: Scanner::new(),
            fast_pre: None,
        }
    }

    /// The whole tree of `text`, as one patch for a client that holds none of it.
    pub fn read(text: &str, options: MarkdownOptions) -> Patch {
        vec![MdOp::List {
            p: Vec::new(),
            at: 0,
            c: values(parse(text, options, 0)),
        }]
    }

    pub fn push(&mut self, more: &str) {
        let prev_len = self.text.len();
        self.text.push_str(more);
        self.units += units(more);
        let reaches = self.scan.advance(&self.text, prev_len, self.options.gfm);
        if !self.whole && reaches {
            self.whole = true;
            self.base = 0;
            self.settled = 0;
            self.open.clear();
            self.fast_pre = None;
        }
    }

    /// How long the text is, in the UTF-16 units clients count it in.
    pub fn units(&self) -> usize {
        self.units
    }

    /// What the tree's clients must change to match the text so far, if anything.
    pub fn patch(&mut self) -> Patch {
        if let Some(patch) = self.patch_fast_pre() {
            return patch;
        }
        self.reread(true)
    }

    /// What the tree's clients must change once the message is done, to draw the markup that
    /// `patch` closed early as it was written.
    pub fn exact(&mut self) -> Patch {
        if !self.mended {
            return Vec::new();
        }
        self.reread(false)
    }

    fn patch_fast_pre(&mut self) -> Option<Patch> {
        let fast = self.fast_pre.as_mut()?;
        if self.text.len() == fast.synced_len {
            return Some(Vec::new());
        }
        let pending = self.text.get(fast.synced_len..)?;
        if pending.contains('\r') {
            self.fast_pre = None;
            return None;
        }
        let last_nl = pending.rfind('\n');
        if last_nl.is_some() || !fast.line_disqualified {
            let check = self.text.get(fast.line_start..)?;
            for line in check.split_inclusive('\n') {
                if closes_top_fence(line, fast.mark, fast.width) {
                    self.fast_pre = None;
                    return None;
                }
            }
            if let Some(rel) = last_nl {
                fast.line_start = fast.synced_len + rel + 1;
            }
            fast.line_disqualified = line_cannot_close(
                self.text.get(fast.line_start..).unwrap_or_default(),
                fast.mark,
            );
        }
        let block_ix = self.settled + self.open.len().checked_sub(1)?;
        let Value::String(v) = self.open.last_mut()?.get_mut("v")? else {
            self.fast_pre = None;
            return None;
        };
        let old_synthetic = fast.synthetic_nl;
        let old_body_utf16 = fast.body_utf16;
        fast.body_utf16 += units(pending);
        fast.synced_len = self.text.len();
        if old_synthetic && pending == "\n" {
            fast.synthetic_nl = false;
            return Some(Vec::new());
        }
        if old_synthetic {
            v.pop();
        }
        v.push_str(pending);
        let new_synthetic = !pending.ends_with('\n');
        if new_synthetic {
            v.push('\n');
        }
        fast.synthetic_nl = new_synthetic;
        let (at, tail) = match pending.strip_prefix('\n') {
            Some(rest) if old_synthetic => (old_body_utf16 + 1, rest),
            _ => (old_body_utf16, pending),
        };
        let mut s = String::with_capacity(tail.len() + usize::from(new_synthetic));
        s.push_str(tail);
        if new_synthetic {
            s.push('\n');
        }
        Some(vec![MdOp::Text {
            p: vec![block_ix],
            at,
            s,
        }])
    }

    fn arm_fast_pre(&mut self, old_base: usize, last_start: Option<usize>) {
        if self.text.ends_with('\r') {
            return;
        }
        let Some(fence) = self.scan.open_top_fence(&self.text) else {
            return;
        };
        let Some(last_start) = last_start else {
            return;
        };
        if old_base + last_start != fence.open_at {
            return;
        }
        let Some(last_block) = self.open.last() else {
            return;
        };
        if last_block.get("t").and_then(Value::as_str) != Some("pre") {
            return;
        }
        let Some(v) = last_block.get("v").and_then(Value::as_str) else {
            return;
        };
        let synthetic_nl = !self.text.ends_with('\n');
        let body_utf16 = units(v).saturating_sub(usize::from(synthetic_nl));
        let line_start = self.scan.line_start;
        let line_disqualified =
            line_cannot_close(self.text.get(line_start..).unwrap_or_default(), fence.mark);
        self.fast_pre = Some(FastPre {
            mark: fence.mark,
            width: fence.width,
            synced_len: self.text.len(),
            line_start,
            line_disqualified,
            body_utf16,
            synthetic_nl,
        });
    }

    fn reread(&mut self, mending: bool) -> Patch {
        self.fast_pre = None;
        let text = self.text.get(self.base..).unwrap_or_default();
        let (mut blocks, starts) = parse_blocks(text, self.options);
        let last = starts.len().saturating_sub(1);
        let mended = starts
            .last()
            .filter(|_| mending && !matches!(blocks.last(), Some(MdElement::Pre { .. })))
            .and_then(|&start| Some((text.get(..start)?, mend(text.get(start..)?)?)));
        self.mended = mended.is_some();
        if let Some((before, block)) = mended {
            blocks.truncate(last);
            if self.whole {
                blocks.extend(parse(&(before.to_owned() + &block), self.options, last));
            } else {
                blocks.extend(parse(&block, self.options, 0));
            }
        }
        let last_start = starts.last().copied();
        let blocks = values(blocks);
        let patch = diff(&self.open, &blocks, self.settled);
        let closed = if self.whole {
            0
        } else {
            starts.len().saturating_sub(OPEN_BLOCKS)
        };
        let old_base = self.base;
        if let Some(&start) = starts.get(closed).filter(|_| closed > 0) {
            let mut next = self.base + start;
            while next > self.base
                && self
                    .text
                    .as_bytes()
                    .get(next - 1)
                    .is_some_and(|&b| b == b' ' || b == b'\t')
            {
                next -= 1;
            }
            self.base = next;
            self.settled += closed;
        }
        self.open = blocks;
        self.open.drain(..closed);
        if mending && !self.mended {
            self.arm_fast_pre(old_base, last_start);
        }
        patch
    }
}
