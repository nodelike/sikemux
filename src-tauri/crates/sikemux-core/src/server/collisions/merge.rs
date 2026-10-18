//! Three-way merge of a file by lines: what a writer meant to change since
//! the copy it worked from, laid over what the file holds now. Where both
//! changed the same lines, what the file holds now is kept, unless the
//! writer's version already holds all of it.

use std::ops::Range;

use crate::server::edits::{replaced, Replaced};

pub(crate) struct Merged {
    pub text: String,
    /// Lines of `text`, counted from 0, where the writer's version was left
    /// out in favour of what the file held; empty where that was nothing.
    pub kept: Vec<Range<usize>>,
}

pub(crate) fn lines(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

/// `ours`, written from `base`, merged with `theirs`, which the file became
/// meanwhile.
pub(crate) fn merge(base: &str, ours: &str, theirs: &str) -> Merged {
    let base = lines(base);
    let ours = lines(ours);
    let theirs = lines(theirs);
    let mine = replaced(&base, &ours);
    let other = replaced(&base, &theirs);
    let (mut i, mut j) = (0, 0);
    let mut at = 0;
    let mut out: Vec<&str> = Vec::new();
    let mut kept = Vec::new();
    while i < mine.len() || j < other.len() {
        let mine_first =
            j >= other.len() || (i < mine.len() && mine[i].old.start <= other[j].old.start);
        let first = if mine_first { &mine[i] } else { &other[j] };
        let mut span = first.old.clone();
        let (from_i, from_j) = (i, j);
        if mine_first {
            i += 1;
        } else {
            j += 1;
        }
        loop {
            if i < mine.len() && meets(&mine[i].old, &span) {
                span.end = span.end.max(mine[i].old.end);
                i += 1;
            } else if j < other.len() && meets(&other[j].old, &span) {
                span.end = span.end.max(other[j].old.end);
                j += 1;
            } else {
                break;
            }
        }
        out.extend(&base[at..span.start]);
        let base_part = &base[span.clone()];
        let ours_part = applied(&base, &ours, &mine[from_i..i], &span);
        let theirs_part = applied(&base, &theirs, &other[from_j..j], &span);
        if from_j == j || ours_part == theirs_part || holds(&ours_part, &theirs_part, base_part) {
            out.extend(ours_part);
        } else if from_i == i {
            out.extend(theirs_part);
        } else {
            let start = out.len();
            out.extend(theirs_part);
            kept.push(start..out.len());
        }
        at = span.end;
    }
    out.extend(&base[at..]);
    Merged {
        text: out.concat(),
        kept,
    }
}

/// Whether a change to `other` falls on the same lines as the changes
/// gathered in `span`. Two insertions at one place meet, since nothing says
/// which goes first; a change ending where another starts does not.
fn meets(other: &Range<usize>, span: &Range<usize>) -> bool {
    (other.start < span.end && span.start < other.end)
        || (other.start == span.start && other.is_empty() && span.is_empty())
}

/// The lines `side` has in place of `base`'s `span`, given its changes there.
fn applied<'a>(
    base: &[&'a str],
    side: &[&'a str],
    changes: &[Replaced],
    span: &Range<usize>,
) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut at = span.start;
    for change in changes {
        out.extend(&base[at..change.old.start]);
        out.extend(&side[change.new.clone()]);
        at = change.old.end;
    }
    out.extend(&base[at..span.end]);
    out
}

/// Whether `ours` keeps everything `theirs` did to `base`: all of `theirs`'
/// lines in order, and none of the lines it took out.
fn holds(ours: &[&str], theirs: &[&str], base: &[&str]) -> bool {
    let taken_out: Vec<&str> = replaced(base, theirs)
        .into_iter()
        .flat_map(|change| base[change.old].to_vec())
        .collect();
    replaced(theirs, ours).iter().all(|change| {
        change.old.is_empty()
            && ours[change.new.clone()]
                .iter()
                .all(|line| !taken_out.contains(line))
    })
}
