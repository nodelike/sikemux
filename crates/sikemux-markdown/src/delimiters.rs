//! LaTeX's own math delimiters, `\(…\)` and `\[…\]`, written as the dollar signs the parser
//! reads math between, leaving code as it is.

/// `text` with each closed `\(…\)` as `$…$` and `\[…\]` as `$$…$$`, and where each byte of
/// the result came from in `text`; none when it has neither.
pub(crate) fn as_dollars(text: &str) -> Option<(String, Moved)> {
    if !text.contains("\\(") && !text.contains("\\[") {
        return None;
    }
    let mut out = String::with_capacity(text.len());
    let mut moved = Moved(Vec::new());
    let mut fenced = false;
    let mut at = 0;
    while let Some(rest) = text.get(at..).filter(|rest| !rest.is_empty()) {
        let before = text.get(..at).unwrap_or_default();
        if before.is_empty() || before.ends_with('\n') {
            let opened = rest.trim_start_matches([' ', '\t']);
            if opened.starts_with("```") || opened.starts_with("~~~") {
                fenced = !fenced;
            }
        }
        if fenced {
            let line = rest.find('\n').map_or(rest.len(), |end| end + 1);
            out.push_str(rest.get(..line).unwrap_or(rest));
            at += line;
            continue;
        }
        if rest.starts_with('`') {
            let run = rest.len() - rest.trim_start_matches('`').len();
            let ticks = rest.get(..run).unwrap_or_default();
            let end = (rest.get(run..))
                .and_then(|after| after.find(ticks))
                .map_or(run, |end| run + end + run);
            out.push_str(rest.get(..end).unwrap_or(rest));
            at += end;
            continue;
        }
        let pair = if rest.starts_with("\\(") {
            Some(("\\)", "$"))
        } else if rest.starts_with("\\[") {
            Some(("\\]", "$$"))
        } else {
            None
        };
        let found = pair
            .filter(|_| !before.ends_with('\\'))
            .and_then(|(closer, dollars)| {
                let inner = rest.get(2..)?;
                let end = inner.find(closer)?;
                let math = inner.get(..end)?;
                (!math.trim().is_empty() && !math.contains("\n\n")).then_some((dollars, math, end))
            });
        let Some((dollars, math, end)) = found else {
            let char = rest.chars().next().unwrap_or_default();
            out.push(char);
            at += char.len_utf8();
            continue;
        };
        let (kept, lead) = match dollars {
            "$" => (math.trim(), math.len() - math.trim_start().len()),
            _ => (math, 0),
        };
        out.push_str(dollars);
        moved.shift(out.len(), at + 2 + lead);
        out.push_str(kept);
        out.push_str(dollars);
        at += 2 + end + 2;
        moved.shift(out.len(), at);
    }
    Some((out, moved))
}

/// Where the bytes of a rewritten text came from: from each byte named on, the byte of the
/// original it maps to, counting on from there.
pub(crate) struct Moved(Vec<(usize, usize)>);

impl Moved {
    fn shift(&mut self, written: usize, original: usize) {
        self.0.push((written, original));
    }

    /// The byte of the original text the rewritten text's byte `at` came from.
    pub(crate) fn original(&self, at: usize) -> usize {
        match self.0.iter().rev().find(|(written, _)| *written <= at) {
            Some((written, original)) => original + (at - written),
            None => at,
        }
    }
}
