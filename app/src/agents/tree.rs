use std::collections::{HashMap, HashSet};

use crate::agents::rows::AgentRow;

/// How deep a row is indented at most; rows below that stay at this depth and name their lead.
pub(crate) const DEEPEST: usize = 3;

/// One row of an agent tree: the agent, how deep it hangs, and the lines that hang it there.
pub(crate) struct Placed {
    pub row: AgentRow,
    pub hang: Hang,
    /// How many agents hang below it at any depth, when it has any.
    pub below: usize,
}

/// Where a row hangs: its depth and the line drawn at each level to its left.
#[derive(Clone, Default)]
pub(crate) struct Hang {
    pub depth: usize,
    pub lines: Vec<Line>,
    /// Its lead's id, when the row does not hang under its lead's row.
    pub lead_apart: Option<String>,
    /// It has rows hanging under it right below, so a line drops from its mark.
    pub open: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Line {
    /// Nothing at this level.
    Blank,
    /// A spine passing by to a later row of an agent above.
    Through,
    /// A tick to this row, with the spine going on to a later sibling.
    Tick,
    /// A corner to this row, the last of its siblings.
    Corner,
}

/// `rows` laid out as trees by each row's lead, every root at `root` depth. Siblings keep the
/// order they come in. A row whose lead is not among `rows` is a root and names its lead; the
/// rows under a lead in `folded` are left out.
pub(crate) fn tree(rows: &[AgentRow], root: usize, folded: &HashSet<String>) -> Vec<Placed> {
    let at: HashMap<&str, usize> = (rows.iter().enumerate())
        .map(|(ix, row)| (row.id.as_str(), ix))
        .collect();
    let parent: Vec<Option<usize>> = (rows.iter().enumerate())
        .map(|(ix, row)| {
            let lead = *at.get(row.lead.as_deref()?)?;
            (lead != ix).then_some(lead)
        })
        .collect();
    let mut children = vec![Vec::new(); rows.len()];
    for (ix, lead) in parent.iter().enumerate() {
        if let Some(lead) = lead {
            children[*lead].push(ix);
        }
    }
    let mut below = vec![0; rows.len()];
    let mut placed = vec![false; rows.len()];
    let mut order: Vec<(usize, usize)> = Vec::with_capacity(rows.len());
    fn walk(
        ix: usize,
        depth: usize,
        children: &[Vec<usize>],
        placed: &mut [bool],
        below: &mut [usize],
        order: &mut Vec<(usize, usize)>,
    ) -> usize {
        placed[ix] = true;
        order.push((ix, depth));
        let mut count = 0;
        for &child in &children[ix] {
            if !placed[child] {
                count += 1 + walk(child, depth + 1, children, placed, below, order);
            }
        }
        below[ix] = count;
        count
    }
    for (ix, _) in parent.iter().enumerate().filter(|(_, lead)| lead.is_none()) {
        walk(ix, 0, &children, &mut placed, &mut below, &mut order);
    }
    for ix in 0..rows.len() {
        if !placed[ix] {
            walk(ix, 0, &children, &mut placed, &mut below, &mut order);
        }
    }
    let mut shown: Vec<(usize, usize)> = Vec::with_capacity(order.len());
    let mut hidden_below: Option<usize> = None;
    for (ix, depth) in order {
        match hidden_below {
            Some(fold) if depth > fold => continue,
            _ => hidden_below = None,
        }
        if below[ix] > 0 && folded.contains(&rows[ix].id) {
            hidden_below = Some(depth);
        }
        shown.push((ix, depth));
    }
    let depths: Vec<usize> = (shown.iter())
        .map(|&(_, depth)| (root + depth).min(DEEPEST))
        .collect();
    let lines = lines_of(&depths);
    (shown
        .into_iter()
        .zip(depths.iter().copied())
        .zip(lines)
        .enumerate())
    .map(|(at, (((ix, depth), drawn), lines))| {
        let row = &rows[ix];
        let deeper = root + depth > drawn;
        let lead_apart = match parent[ix] {
            Some(_) if !deeper => None,
            _ => row.lead.clone(),
        };
        Placed {
            row: row.clone(),
            hang: Hang {
                depth: drawn,
                lines,
                lead_apart,
                open: depths.get(at + 1).is_some_and(|next| *next > drawn),
            },
            below: below[ix],
        }
    })
    .collect()
}

/// The lines to the left of each row of a tree drawn at `depths`, top to bottom.
pub(crate) fn lines_of(depths: &[usize]) -> Vec<Vec<Line>> {
    let deepest = depths.iter().copied().max().unwrap_or(0);
    // Whether a later row hangs at each level before the tree closes back past it.
    let mut later = vec![false; deepest];
    let mut lines = vec![Vec::new(); depths.len()];
    for (ix, &depth) in depths.iter().enumerate().rev() {
        lines[ix] = (0..depth)
            .map(|level| match (level + 1 == depth, later[level]) {
                (true, true) => Line::Tick,
                (true, false) => Line::Corner,
                (false, true) => Line::Through,
                (false, false) => Line::Blank,
            })
            .collect();
        for open in later.iter_mut().skip(depth) {
            *open = false;
        }
        if depth > 0 {
            later[depth - 1] = true;
        }
    }
    lines
}

/// The ids of `rows` that hang, at any depth, under one of `tops`, with the tops themselves.
pub(crate) fn under(rows: &[AgentRow], tops: &HashSet<&str>) -> HashSet<String> {
    let lead_of: HashMap<&str, &str> = (rows.iter())
        .filter_map(|row| Some((row.id.as_str(), row.lead.as_deref()?)))
        .collect();
    (rows.iter())
        .filter(|row| {
            let mut at = row.id.as_str();
            for _ in 0..=rows.len() {
                if tops.contains(at) {
                    return true;
                }
                match lead_of.get(at) {
                    Some(lead) => at = lead,
                    None => return false,
                }
            }
            false
        })
        .map(|row| row.id.clone())
        .collect()
}

/// The ids of the rows `from` hang under, at any depth.
pub(crate) fn above(rows: &[AgentRow], from: &HashSet<&str>) -> HashSet<String> {
    let lead_of: HashMap<&str, &str> = (rows.iter())
        .filter_map(|row| Some((row.id.as_str(), row.lead.as_deref()?)))
        .collect();
    let mut leads = HashSet::new();
    for &start in from {
        let mut at = start;
        for _ in 0..rows.len() {
            match lead_of.get(at) {
                Some(&lead) if leads.insert(lead.to_owned()) => at = lead,
                _ => break,
            }
        }
    }
    leads
}

#[cfg(test)]
mod tests {
    use super::{lines_of, Line::*};

    #[test]
    fn a_spine_runs_past_a_deeper_team_to_the_next_sibling() {
        let lines = lines_of(&[1, 2, 3, 2, 1, 2]);
        assert_eq!(lines[0], vec![Tick]);
        assert_eq!(lines[1], vec![Through, Tick]);
        assert_eq!(lines[2], vec![Through, Through, Corner]);
        assert_eq!(lines[3], vec![Through, Corner]);
        assert_eq!(lines[4], vec![Corner]);
        assert_eq!(lines[5], vec![Blank, Corner]);
    }
}
