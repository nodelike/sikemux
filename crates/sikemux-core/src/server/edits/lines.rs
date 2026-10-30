//! Which lines of a file an edit changed, as `[start, count, removed]` on the
//! file after it: `count` lines from line `start`, counted from 1, stand where
//! `removed` lines stood before.

pub(crate) type Lines = Vec<[u64; 3]>;

/// The most changes a diff works out line by line before it calls everything
/// between the first and last change one block.
const MAX_CHANGES: usize = 1000;

#[derive(Clone, Copy)]
enum Op {
    Same,
    Added,
    Removed,
}

fn blocks(ops: impl IntoIterator<Item = Op>, first: u64) -> Lines {
    let mut out = Vec::new();
    let mut open: Option<[u64; 3]> = None;
    let mut line = first;
    for op in ops {
        match op {
            Op::Same => {
                out.extend(open.take());
                line += 1;
            }
            Op::Added => {
                open.get_or_insert([line, 0, 0])[1] += 1;
                line += 1;
            }
            Op::Removed => {
                open.get_or_insert([line, 0, 0])[2] += 1;
            }
        }
    }
    out.extend(open);
    out
}

/// The lines of `new` that differ from `old`, numbered from `first`.
pub(crate) fn changed(old: &str, new: &str, first: u64) -> Lines {
    let old: Vec<&str> = old.lines().collect();
    let new: Vec<&str> = new.lines().collect();
    let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let (old, new) = (&old[prefix..], &new[prefix..]);
    let suffix = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let (old, new) = (&old[..old.len() - suffix], &new[..new.len() - suffix]);
    let ops = diff(old, new).unwrap_or_else(|| {
        std::iter::repeat_n(Op::Removed, old.len())
            .chain(std::iter::repeat_n(Op::Added, new.len()))
            .collect()
    });
    blocks(ops, first + prefix as u64)
}

/// A run of the old text's lines and the lines that stand in its place in the
/// new one, both counted from 0. Either may be empty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Replaced {
    pub old: std::ops::Range<usize>,
    pub new: std::ops::Range<usize>,
}

/// Each run of `old` that `new` replaces, in order.
pub(crate) fn replaced<T: AsRef<str>>(old: &[T], new: &[T]) -> Vec<Replaced> {
    let old: Vec<&str> = old.iter().map(AsRef::as_ref).collect();
    let new: Vec<&str> = new.iter().map(AsRef::as_ref).collect();
    let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let (a, b) = (
        &old[prefix..old.len() - suffix],
        &new[prefix..new.len() - suffix],
    );
    let ops = diff(a, b).unwrap_or_else(|| {
        std::iter::repeat_n(Op::Removed, a.len())
            .chain(std::iter::repeat_n(Op::Added, b.len()))
            .collect()
    });
    let mut out = Vec::new();
    let mut open: Option<Replaced> = None;
    let (mut at_old, mut at_new) = (prefix, prefix);
    for op in ops {
        match op {
            Op::Same => {
                out.extend(open.take());
                at_old += 1;
                at_new += 1;
            }
            Op::Removed => {
                open.get_or_insert(Replaced {
                    old: at_old..at_old,
                    new: at_new..at_new,
                })
                .old
                .end += 1;
                at_old += 1;
            }
            Op::Added => {
                open.get_or_insert(Replaced {
                    old: at_old..at_old,
                    new: at_new..at_new,
                })
                .new
                .end += 1;
                at_new += 1;
            }
        }
    }
    out.extend(open);
    out
}

/// The lines a unified diff hunk changed, given where it starts in the new
/// file and its lines, each marked ` `, `+` or `-`.
pub(crate) fn hunk<'a>(new_start: u64, lines: impl IntoIterator<Item = &'a str>) -> Lines {
    let ops = lines
        .into_iter()
        .filter_map(|line| match line.as_bytes().first() {
            Some(b'+') => Some(Op::Added),
            Some(b'-') => Some(Op::Removed),
            Some(b'\\') => None,
            _ => Some(Op::Same),
        });
    blocks(ops, new_start)
}

/// Myers' shortest edit script from `a` to `b`, or none when it takes more
/// than [`MAX_CHANGES`] changes.
fn diff(a: &[&str], b: &[&str]) -> Option<Vec<Op>> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    let limit = (a.len() + b.len()).min(MAX_CHANGES) as isize;
    let offset = limit + 1;
    let mut v = vec![0isize; (2 * limit + 3) as usize];
    let mut trace: Vec<Vec<isize>> = Vec::new();
    for d in 0..=limit {
        trace.push(v[(offset - d - 1) as usize..=(offset + d + 1) as usize].to_vec());
        for k in (-d..=d).step_by(2) {
            let at = |k: isize| v[(offset + k) as usize];
            let mut x = if k == -d || (k != d && at(k - 1) < at(k + 1)) {
                at(k + 1)
            } else {
                at(k - 1) + 1
            };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[(offset + k) as usize] = x;
            if x >= n && y >= m {
                return Some(backtrack(&trace, n, m));
            }
        }
    }
    None
}

fn backtrack(trace: &[Vec<isize>], n: isize, m: isize) -> Vec<Op> {
    let (mut x, mut y) = (n, m);
    let mut ops = Vec::new();
    for (d, v) in trace.iter().enumerate().rev() {
        let d = d as isize;
        let at = |k: isize| v[(k + d + 1) as usize];
        let k = x - y;
        let prev_k = if k == -d || (k != d && at(k - 1) < at(k + 1)) {
            k + 1
        } else {
            k - 1
        };
        let prev_x = at(prev_k);
        let prev_y = prev_x - prev_k;
        while x > prev_x && y > prev_y {
            ops.push(Op::Same);
            x -= 1;
            y -= 1;
        }
        if d > 0 {
            ops.push(if x == prev_x { Op::Added } else { Op::Removed });
        }
        x = prev_x;
        y = prev_y;
    }
    ops.reverse();
    ops
}
