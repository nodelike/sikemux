//! A run's jobs laid out as the code host draws them, and a job's log split
//! into the steps that wrote it.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;
use sikemux_wire::codehost::{
    GraphBox, GraphEdge, GraphJob, Job, JobGraph, JobLog, JobLogLine, LogMark, LogSection,
    OpenJobLog, Outcome, Step,
};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use super::ansi::spans_of;

/// What the graph needs from one entry under a workflow file's `jobs:`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorkflowJob {
    key: String,
    name: Option<String>,
    needs: Vec<String>,
    /// The workflow this job calls instead of running steps, as written after `uses:`.
    uses: Option<String>,
}

impl WorkflowJob {
    fn display(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.key)
    }
}

/// Each workflow file's jobs by its path, or none when it could not be read.
pub(crate) type WorkflowFiles = HashMap<String, Option<Vec<WorkflowJob>>>;

/// Called workflows nest at most this deep.
pub(crate) const CALLS_AT_MOST: usize = 4;

static KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([A-Za-z_][\w-]*)\s*:(?:\s+(.*))?$").unwrap());
static JOBS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^jobs\s*:$").unwrap());
static EXPRESSION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)\$\{\{.*?\}\}").unwrap());
static MATRIX_EXPRESSION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)\s*\(\s*\$\{\{.*?\}\}[^)]*\)").unwrap());
static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s{2,}").unwrap());
static MARKUP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^##\[(group|endgroup|section|command|error|warning|notice|debug)\]").unwrap()
});

fn without_comment(raw: &str) -> &str {
    let mut quote = None;
    let mut before: Option<char> = None;
    for (at, char) in raw.char_indices() {
        let opens = before.is_none_or(|before| before.is_whitespace() || "[{,".contains(before));
        if let Some(open) = quote {
            if char == open {
                quote = None;
            }
        } else if (char == '"' || char == '\'') && opens {
            quote = Some(char);
        } else if char == '#' && before.is_none_or(char::is_whitespace) {
            return &raw[..at];
        }
        before = Some(char);
    }
    raw
}

struct Line<'a> {
    indent: usize,
    text: &'a str,
}

fn lines_of(text: &str) -> Vec<Line<'_>> {
    text.lines()
        .filter_map(|raw| {
            let body = without_comment(raw).trim_end();
            let trimmed = body.trim_start();
            (!trimmed.is_empty()).then(|| Line {
                indent: body.len() - trimmed.len(),
                text: trimmed,
            })
        })
        .collect()
}

fn scalar(value: &str) -> String {
    let text = value.trim();
    if text.len() >= 2 && text.starts_with('\'') && text.ends_with('\'') {
        return text[1..text.len() - 1].replace("''", "'");
    }
    if text.len() >= 2 && text.starts_with('"') && text.ends_with('"') {
        return serde_json::from_str(text).unwrap_or_else(|_| text[1..text.len() - 1].to_owned());
    }
    text.to_owned()
}

fn list_of(flow: &str) -> Vec<String> {
    let flow = flow.strip_prefix('[').unwrap_or(flow);
    let flow = flow.strip_suffix(']').unwrap_or(flow);
    flow.split(',')
        .map(scalar)
        .filter(|each| !each.is_empty())
        .collect()
}

/// Each job's key, `name`, `needs` and `uses` in a workflow file, or none when
/// it has no `jobs:`. It reads only what GitHub's own files write for those,
/// not the whole of YAML.
pub(crate) fn read_jobs(text: &str) -> Option<Vec<WorkflowJob>> {
    let lines = lines_of(text);
    let start = lines
        .iter()
        .position(|line| line.indent == 0 && JOBS.is_match(line.text))?;
    let job_indent = lines.get(start + 1).map_or(0, |line| line.indent);
    if job_indent == 0 {
        return Some(Vec::new());
    }
    let mut jobs: Vec<WorkflowJob> = Vec::new();
    let mut in_job = false;
    let mut field_indent: Option<usize> = None;
    let mut at = start + 1;
    while at < lines.len() {
        let line = &lines[at];
        if line.indent < job_indent {
            break;
        }
        if line.indent == job_indent {
            in_job = false;
            if let Some(key) = KEY.captures(line.text).map(|found| found[1].to_owned()) {
                jobs.push(WorkflowJob {
                    key,
                    name: None,
                    needs: Vec::new(),
                    uses: None,
                });
                in_job = true;
            }
            field_indent = None;
            at += 1;
            continue;
        }
        let Some(job) = jobs.last_mut().filter(|_| in_job) else {
            at += 1;
            continue;
        };
        let field_at = *field_indent.get_or_insert(line.indent);
        let field = KEY.captures(line.text).filter(|_| line.indent == field_at);
        let Some(field) = field else {
            at += 1;
            continue;
        };
        let value = field.get(2).map_or("", |value| value.as_str());
        match &field[1] {
            "name" if !value.starts_with(['|', '>']) => {
                job.name = Some(scalar(value)).filter(|name| !name.is_empty());
            }
            "uses" => job.uses = Some(scalar(value)).filter(|uses| !uses.is_empty()),
            "needs" if value.starts_with('[') => {
                let mut flow = value.to_owned();
                while !flow.contains(']') && at + 1 < lines.len() && lines[at + 1].indent > field_at
                {
                    at += 1;
                    flow.push(' ');
                    flow.push_str(lines[at].text);
                }
                job.needs = list_of(&flow);
            }
            "needs" if !value.is_empty() => job.needs = vec![scalar(value)],
            "needs" => {
                let mut needs = Vec::new();
                while let Some(next) = lines.get(at + 1) {
                    if next.indent < field_at || !next.text.starts_with('-') {
                        break;
                    }
                    needs.push(scalar(&next.text[1..]));
                    at += 1;
                }
                job.needs = needs.into_iter().filter(|need| !need.is_empty()).collect();
            }
            _ => {}
        }
        at += 1;
    }
    Some(jobs)
}

/// The repository path of a workflow called from the same repository, such
/// as `./.github/workflows/ci.yml`.
pub(crate) fn called_path(uses: &str) -> Option<&str> {
    uses.strip_prefix("./")?.split('@').next()
}

/// The workflow files `jobs` call, which are read next.
pub(crate) fn calls(jobs: &[WorkflowJob]) -> impl Iterator<Item = &str> {
    jobs.iter()
        .filter_map(|job| job.uses.as_deref().and_then(called_path))
}

/// A matrix job's values are in parentheses and may hold " / " themselves, so
/// only a split outside them counts.
fn split_name(name: &str) -> (Option<&str>, &str) {
    let mut depth = 0usize;
    for (at, char) in name.char_indices() {
        match char {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ if depth == 0 && at > 0 && name[at..].starts_with(" / ") => {
                return (Some(&name[..at]), &name[at + 3..]);
            }
            _ => {}
        }
    }
    (None, name)
}

fn millis(value: Option<&str>) -> Option<f64> {
    let at = OffsetDateTime::parse(value?, &Rfc3339).ok()?;
    Some((at.unix_timestamp_nanos() / 1_000_000) as f64)
}

#[derive(Clone)]
struct Named<'a> {
    job: &'a Job,
    name: String,
    order: usize,
}

struct Group<'a> {
    key: String,
    jobs: Vec<&'a Job>,
}

/// The list of jobs does not say which job waited for which, so without the
/// workflow file the stages are read off the clock: a group that only started
/// once everything before it had finished was waiting on it. Jobs that have
/// not started yet come last.
fn stages_of<'a>(named: &[Named<'a>]) -> Vec<Vec<Group<'a>>> {
    struct Unit<'a> {
        group: Group<'a>,
        start: Option<f64>,
        end: Option<f64>,
        order: usize,
    }
    let mut units: Vec<Unit> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for (order, Named { job, name, .. }) in named.iter().enumerate() {
        let (group, _) = split_name(name);
        let key = group.map_or_else(|| format!("job:{}", job.id), str::to_owned);
        let at = *index.entry(key.clone()).or_insert_with(|| {
            units.push(Unit {
                group: Group {
                    key,
                    jobs: Vec::new(),
                },
                start: None,
                end: None,
                order,
            });
            units.len() - 1
        });
        let unit = &mut units[at];
        unit.group.jobs.push(job);
        if let Some(start) = millis(job.started_at.as_deref()) {
            unit.start = Some(unit.start.map_or(start, |known| known.min(start)));
        }
        let end = millis(job.completed_at.as_deref());
        if job.status != "completed" {
            unit.end = Some(f64::INFINITY);
        } else if let Some(end) = end {
            unit.end = Some(unit.end.map_or(end, |known| known.max(end)));
        }
    }
    let (mut started, mut waiting): (Vec<Unit>, Vec<Unit>) =
        units.into_iter().partition(|unit| unit.start.is_some());
    started.sort_by(|a, b| {
        a.start
            .unwrap_or_default()
            .total_cmp(&b.start.unwrap_or_default())
            .then(a.order.cmp(&b.order))
    });
    waiting.sort_by_key(|unit| unit.order);

    let mut stages = Vec::new();
    let mut current: Vec<Group> = Vec::new();
    let mut current_end = f64::NEG_INFINITY;
    for unit in started {
        if !current.is_empty() && unit.start.unwrap_or_default() >= current_end {
            stages.push(std::mem::take(&mut current));
            current_end = f64::NEG_INFINITY;
        }
        current_end = current_end.max(unit.end.unwrap_or(f64::INFINITY));
        current.push(unit.group);
    }
    if !current.is_empty() {
        stages.push(current);
    }
    if !waiting.is_empty() {
        stages.push(waiting.into_iter().map(|unit| unit.group).collect());
    }
    stages
}

/// A name with its `${{ … }}` parts taken out, for a job that has not run yet.
fn plain_name(name: &str) -> String {
    let plain = MATRIX_EXPRESSION.replace_all(name, "");
    let plain = EXPRESSION.replace_all(&plain, "");
    let plain = SPACES.replace_all(&plain, " ");
    let plain = plain.trim();
    if plain.is_empty() {
        name.to_owned()
    } else {
        plain.to_owned()
    }
}

/// GitHub fills in a name's expressions, or adds a matrix job's values after
/// it in parentheses.
enum Matcher {
    Plain(String),
    Pattern(Regex),
}

impl Matcher {
    fn of(display: &str) -> Self {
        if !EXPRESSION.is_match(display) {
            return Self::Plain(display.to_owned());
        }
        let parts: Vec<String> = EXPRESSION.split(display).map(regex::escape).collect();
        Regex::new(&format!("^(?s){}$", parts.join(".*?")))
            .map_or_else(|_| Self::Plain(display.to_owned()), Self::Pattern)
    }

    fn test(&self, name: &str) -> bool {
        match self {
            Self::Plain(display) => {
                name == display
                    || (name.starts_with(&format!("{display} (")) && name.ends_with(')'))
            }
            Self::Pattern(pattern) => pattern.is_match(name),
        }
    }
}

fn matrix_label(name: &str, title: &str) -> String {
    name.strip_prefix(title)
        .and_then(|rest| rest.strip_prefix(" ("))
        .and_then(|rest| rest.strip_suffix(')'))
        .unwrap_or(name)
        .to_owned()
}

struct Part {
    id: String,
    title: Option<String>,
    jobs: Vec<GraphJob>,
    needs: Vec<String>,
    /// Where the run lists it, which is the order GitHub stacks jobs that wait
    /// for nothing.
    order: usize,
}

struct Level {
    parts: Vec<Part>,
    /// The parts nothing else in the same file waits for, which stand for the
    /// file to a job that needs its caller.
    leaves: Vec<String>,
}

fn earliest(records: &[Named]) -> usize {
    records
        .iter()
        .map(|record| record.order)
        .min()
        .unwrap_or(usize::MAX)
}

struct Within<'a> {
    prefix: String,
    label_prefix: String,
    skipped: bool,
    order: usize,
    depth: usize,
    files: &'a WorkflowFiles,
}

/// A called workflow's jobs join the run's own columns, named `Caller / Job`:
/// the ones that wait for nothing in their file wait for what the caller
/// needs, and a job that needs the caller waits for the ones nothing in that
/// file waits for.
fn from_file(named: &[Named], declared: &[WorkflowJob], within: &Within) -> Option<Level> {
    let matches: Vec<Matcher> = declared
        .iter()
        .map(|each| Matcher::of(each.display()))
        .collect();
    let mut own: Vec<Vec<Named>> = vec![Vec::new(); declared.len()];
    let mut called: Vec<Vec<Named>> = vec![Vec::new(); declared.len()];
    for entry in named {
        let (group, label) = split_name(&entry.name);
        let caller = group.and_then(|group| {
            declared
                .iter()
                .zip(&matches)
                .position(|(each, matcher)| each.uses.is_some() && matcher.test(group))
        });
        if let Some(caller) = caller {
            called[caller].push(Named {
                name: label.to_owned(),
                ..entry.clone()
            });
            continue;
        }
        let at = declared
            .iter()
            .position(|each| each.display() == entry.name)
            .or_else(|| matches.iter().position(|matcher| matcher.test(&entry.name)))?;
        own[at].push(entry.clone());
    }

    let mut parts: Vec<Part> = Vec::new();
    let mut entries: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut exits: HashMap<&str, Vec<String>> = HashMap::new();
    for (index, each) in declared.iter().enumerate() {
        let id = format!("{}{}", within.prefix, each.key);
        let records = &own[index];
        let calls = &called[index];
        let title = plain_name(each.display());
        let rank = earliest(records).min(earliest(calls)).min(within.order);
        if let Some(uses) = &each.uses {
            let caller_skipped = within.skipped
                || records
                    .iter()
                    .any(|record| record.job.outcome == Outcome::Skipped);
            let nested = Within {
                prefix: format!("{id}/"),
                label_prefix: format!("{}{title} / ", within.label_prefix),
                skipped: caller_skipped,
                order: rank,
                depth: within.depth + 1,
                files: within.files,
            };
            let file = called_path(uses)
                .and_then(|path| within.files.get(path))
                .and_then(Option::as_ref)
                .filter(|_| nested.depth <= CALLS_AT_MOST);
            let inner = file
                .and_then(|file| from_file(calls, file, &nested))
                .or_else(|| (!calls.is_empty()).then(|| unordered(calls, &nested)));
            if let Some(inner) = inner {
                let base = parts.len();
                entries.insert(
                    &each.key,
                    inner
                        .parts
                        .iter()
                        .enumerate()
                        .filter(|(_, part)| part.needs.is_empty())
                        .map(|(at, _)| base + at)
                        .collect(),
                );
                exits.insert(&each.key, inner.leaves);
                parts.extend(inner.parts);
                continue;
            }
        }
        let jobs = if records.len() > 1 {
            records
                .iter()
                .map(|record| GraphJob {
                    job: Some(record.job.id.clone()),
                    label: matrix_label(&record.name, &title),
                    skipped: false,
                })
                .collect()
        } else {
            let record = records.first();
            vec![GraphJob {
                job: record.map(|record| record.job.id.clone()),
                label: format!(
                    "{}{}",
                    within.label_prefix,
                    record.map_or(title.as_str(), |record| record.name.as_str())
                ),
                skipped: within.skipped,
            }]
        };
        entries.insert(&each.key, vec![parts.len()]);
        exits.insert(&each.key, vec![id.clone()]);
        parts.push(Part {
            title: (records.len() > 1).then(|| format!("{}{title}", within.label_prefix)),
            id,
            jobs,
            needs: Vec::new(),
            order: rank,
        });
    }
    for each in declared {
        let needs: Vec<String> = each
            .needs
            .iter()
            .flat_map(|need| exits.get(need.as_str()).cloned().unwrap_or_default())
            .collect();
        for at in entries.get(each.key.as_str()).into_iter().flatten() {
            parts[*at].needs.extend(needs.iter().cloned());
        }
    }
    let needed: HashSet<&str> = parts
        .iter()
        .flat_map(|part| part.needs.iter().map(String::as_str))
        .collect();
    let leaves = parts
        .iter()
        .filter(|part| !needed.contains(part.id.as_str()))
        .map(|part| part.id.clone())
        .collect();
    Some(Level { parts, leaves })
}

/// A called workflow whose file could not be read: its jobs side by side, all
/// waiting for what the caller needs.
fn unordered(calls: &[Named], within: &Within) -> Level {
    let parts: Vec<Part> = calls
        .iter()
        .map(|call| Part {
            id: format!("{}{}", within.prefix, call.job.id),
            title: None,
            jobs: vec![GraphJob {
                job: Some(call.job.id.clone()),
                label: format!("{}{}", within.label_prefix, call.name),
                skipped: false,
            }],
            needs: Vec::new(),
            order: call.order,
        })
        .collect();
    let leaves = parts.iter().map(|part| part.id.clone()).collect();
    Level { parts, leaves }
}

fn depths_of(parts: &[Part]) -> HashMap<&str, usize> {
    fn depth_of<'a>(
        part: &'a Part,
        by_id: &HashMap<&str, &'a Part>,
        depths: &mut HashMap<&'a str, usize>,
        seen: &mut HashSet<&'a str>,
    ) -> usize {
        if let Some(known) = depths.get(part.id.as_str()) {
            return *known;
        }
        if !seen.insert(&part.id) {
            return 0;
        }
        let mut depth = 0;
        for need in &part.needs {
            if let Some(before) = by_id.get(need.as_str()) {
                depth = depth.max(depth_of(before, by_id, depths, seen) + 1);
            }
        }
        depths.insert(&part.id, depth);
        depth
    }
    let by_id: HashMap<&str, &Part> = parts.iter().map(|part| (part.id.as_str(), part)).collect();
    let mut depths = HashMap::new();
    for part in parts {
        depth_of(part, &by_id, &mut depths, &mut HashSet::new());
    }
    depths
}

/// GitHub's columns: each job one column after the last of the jobs it needs.
/// Jobs in a column that wait for the same jobs and are waited on by the same
/// jobs share a box, and each column is stacked by where the boxes it waits
/// for sit, so the lines stay short and do not cross.
fn layout(parts: &[Part], listed: &HashMap<&str, usize>) -> JobGraph {
    struct Placed {
        drawn: GraphBox,
        depth: usize,
        order: usize,
        needs: HashSet<String>,
    }
    let depths = depths_of(parts);
    let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();
    for part in parts {
        for need in &part.needs {
            dependents.entry(need).or_default().push(&part.id);
        }
    }
    let ids = |list: &[&str]| {
        let mut list: Vec<&str> = list.to_vec();
        list.sort_unstable();
        list.dedup();
        list.join(",")
    };

    let mut boxes: Vec<Placed> = Vec::new();
    let mut by_key: HashMap<String, usize> = HashMap::new();
    let mut box_of: HashMap<&str, usize> = HashMap::new();
    for part in parts {
        let depth = depths.get(part.id.as_str()).copied().unwrap_or_default();
        let key = match part.title {
            Some(_) => part.id.clone(),
            None => {
                let needs: Vec<&str> = part.needs.iter().map(String::as_str).collect();
                let after = dependents
                    .get(part.id.as_str())
                    .cloned()
                    .unwrap_or_default();
                format!("{depth}|{}|{}", ids(&needs), ids(&after))
            }
        };
        let at = match by_key.get(&key) {
            Some(at) => {
                let placed = &mut boxes[*at];
                placed.drawn.jobs.extend(part.jobs.iter().cloned());
                placed.order = placed.order.min(part.order);
                *at
            }
            None => {
                boxes.push(Placed {
                    drawn: GraphBox {
                        id: part.id.clone(),
                        title: part.title.clone(),
                        jobs: part.jobs.clone(),
                    },
                    depth,
                    order: part.order,
                    needs: HashSet::new(),
                });
                by_key.insert(key, boxes.len() - 1);
                boxes.len() - 1
            }
        };
        box_of.insert(&part.id, at);
    }

    let mut edges: Vec<GraphEdge> = Vec::new();
    let mut seen_edges: HashSet<(usize, usize)> = HashSet::new();
    for part in parts {
        let to = box_of[part.id.as_str()];
        for need in &part.needs {
            let Some(&from) = box_of.get(need.as_str()) else {
                continue;
            };
            if from == to || !seen_edges.insert((from, to)) {
                continue;
            }
            edges.push(GraphEdge {
                from: boxes[from].drawn.id.clone(),
                to: boxes[to].drawn.id.clone(),
            });
            let from_id = boxes[from].drawn.id.clone();
            boxes[to].needs.insert(from_id);
        }
    }

    let deepest = boxes.iter().map(|placed| placed.depth).max();
    let mut columns: Vec<Vec<Placed>> = (0..=deepest.unwrap_or(0)).map(|_| Vec::new()).collect();
    if deepest.is_none() {
        columns.clear();
    }
    for placed in boxes {
        columns[placed.depth].push(placed);
    }
    let rank = |row: &GraphJob| {
        row.job.as_deref().map_or(usize::MAX, |job| {
            listed.get(job).copied().unwrap_or_default()
        })
    };
    let mut at: HashMap<String, f64> = HashMap::new();
    let columns = columns
        .into_iter()
        .filter(|column| !column.is_empty())
        .map(|mut column| {
            let weight = |placed: &Placed| {
                if placed.needs.is_empty() {
                    return 0.0;
                }
                let sum: f64 = placed
                    .needs
                    .iter()
                    .map(|need| at.get(need).copied().unwrap_or_default())
                    .sum();
                sum / placed.needs.len() as f64
            };
            column.sort_by(|a, b| weight(a).total_cmp(&weight(b)).then(a.order.cmp(&b.order)));
            let mut top = 0.0;
            for placed in &column {
                at.insert(placed.drawn.id.clone(), top);
                top += (placed.drawn.jobs.len() + usize::from(placed.drawn.title.is_some()) + 1)
                    as f64;
            }
            column
                .into_iter()
                .map(|mut placed| {
                    if placed.drawn.title.is_none() {
                        placed.drawn.jobs.sort_by(|a, b| {
                            rank(a).cmp(&rank(b)).then_with(|| a.label.cmp(&b.label))
                        });
                    }
                    placed.drawn
                })
                .collect()
        })
        .collect();
    JobGraph { columns, edges }
}

/// Without the workflow file, every box in a stage is drawn as waiting for
/// every box in the one before it.
fn by_clock(named: &[Named]) -> JobGraph {
    let columns: Vec<Vec<GraphBox>> = stages_of(named)
        .into_iter()
        .map(|stage| {
            stage
                .into_iter()
                .map(|group| GraphBox {
                    id: group.key,
                    title: None,
                    jobs: group
                        .jobs
                        .iter()
                        .map(|job| GraphJob {
                            job: Some(job.id.clone()),
                            label: job.name.clone(),
                            skipped: false,
                        })
                        .collect(),
                })
                .collect()
        })
        .collect();
    let edges = columns
        .windows(2)
        .flat_map(|pair| {
            pair[0].iter().flat_map(|from| {
                pair[1].iter().map(|to| GraphEdge {
                    from: from.id.clone(),
                    to: to.id.clone(),
                })
            })
        })
        .collect();
    JobGraph { columns, edges }
}

/// The run's jobs laid out as GitHub draws them, by what each one `needs` in
/// the workflow file at `path`, with a called workflow's jobs in the run's own
/// columns. Without the file, or with a job it does not declare, the stages
/// are read off the clock.
pub(crate) fn graph_of(jobs: &[Job], files: &WorkflowFiles, path: Option<&str>) -> JobGraph {
    let named: Vec<Named> = jobs
        .iter()
        .enumerate()
        .map(|(order, job)| Named {
            job,
            name: job.name.clone(),
            order,
        })
        .collect();
    let declared = path
        .and_then(|path| files.get(path))
        .and_then(Option::as_ref);
    let top = Within {
        prefix: String::new(),
        label_prefix: String::new(),
        skipped: false,
        order: usize::MAX,
        depth: 0,
        files,
    };
    match declared.and_then(|declared| from_file(&named, declared, &top)) {
        Some(level) => {
            let listed = jobs
                .iter()
                .enumerate()
                .map(|(order, job)| (job.id.as_str(), order))
                .collect();
            layout(&level.parts, &listed)
        }
        None => by_clock(&named),
    }
}

/// Where each step's part of the log begins. A job's log is one stream, and
/// the only thing tying a line to a step is when it was written.
fn step_starts(stamps: &[Option<f64>], steps: &[Step]) -> HashMap<u64, usize> {
    let mut starts = HashMap::new();
    let mut from = 0;
    for step in steps {
        let Some(begins) = millis(step.started_at.as_deref()) else {
            continue;
        };
        // A step's time is kept to the second and a line's to a fraction of one.
        let floor = (begins / 1000.0).floor() * 1000.0;
        let mut index = from;
        while index < stamps.len() && stamps[index].unwrap_or(f64::NEG_INFINITY) < floor {
            index += 1;
        }
        if index >= stamps.len() {
            break;
        }
        starts.insert(step.number, index);
        from = index;
    }
    starts
}

/// Lines written before the first step began belong to it; a step that never
/// ran holds no lines.
fn sections_of(count: usize, steps: &[Step], starts: &HashMap<u64, usize>) -> Vec<LogSection> {
    if steps.is_empty() {
        return vec![LogSection {
            step: None,
            from: 0,
            to: count,
        }];
    }
    let mut cursor = 0;
    let mut first = true;
    steps
        .iter()
        .enumerate()
        .map(|(index, step)| {
            let Some(&begins) = starts.get(&step.number) else {
                return LogSection {
                    step: Some(step.number),
                    from: cursor,
                    to: cursor,
                };
            };
            let to = steps[index + 1..]
                .iter()
                .find_map(|later| starts.get(&later.number).copied())
                .unwrap_or(count);
            let from = if first { 0 } else { begins };
            first = false;
            cursor = to;
            LogSection {
                step: Some(step.number),
                from,
                to,
            }
        })
        .collect()
}

fn mark_of(name: &str) -> Option<LogMark> {
    Some(match name {
        "group" => LogMark::Group,
        "endgroup" => LogMark::Endgroup,
        "section" => LogMark::Section,
        "command" => LogMark::Command,
        "error" => LogMark::Error,
        "warning" => LogMark::Warning,
        "notice" => LogMark::Notice,
        "debug" => LogMark::Debug,
        _ => return None,
    })
}

/// A job's log split into the steps that wrote it, with the runner's markup
/// read off each line and the script under each `Run` a step opens with,
/// up to its `shell:` line, marked.
pub(crate) fn log_of(log: JobLog, steps: &[Step]) -> OpenJobLog {
    let stamps: Vec<Option<f64>> = log
        .lines
        .iter()
        .map(|line| millis(line.timestamp.as_deref()))
        .collect();
    let starts = step_starts(&stamps, steps);
    let sections = sections_of(log.lines.len(), steps, &starts);
    let mut lines: Vec<JobLogLine> = log
        .lines
        .into_iter()
        .map(|line| {
            let mark = MARKUP
                .captures(&line.text)
                .and_then(|found| mark_of(&found[1]));
            let (text, spans) = spans_of(&MARKUP.replace(&line.text, ""));
            JobLogLine {
                number: line.number,
                text,
                spans,
                mark,
                script: false,
            }
        })
        .collect();
    for section in &sections {
        let mut running = false;
        for line in &mut lines[section.from..section.to] {
            if line.mark == Some(LogMark::Group) && line.text.starts_with("Run ") {
                running = true;
            } else if running
                && (line.mark == Some(LogMark::Endgroup) || line.text.starts_with("shell: "))
            {
                running = false;
            } else if running {
                line.script = true;
            }
        }
    }
    OpenJobLog {
        from: 0,
        total: lines.len(),
        lines,
        sections,
        expired: log.expired,
        truncated: log.truncated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sikemux_wire::codehost::LogLine;

    fn job(
        id: u32,
        name: &str,
        started: Option<String>,
        completed: Option<String>,
        status: &str,
    ) -> Job {
        let done = status == "completed";
        Job {
            id: id.to_string(),
            name: name.to_owned(),
            status: status.to_owned(),
            conclusion: done.then(|| "success".to_owned()),
            outcome: if done {
                Outcome::Success
            } else {
                Outcome::Running
            },
            started_at: started,
            completed_at: completed,
            runner: None,
            url: None,
            check_run_id: None,
            steps: Vec::new(),
        }
    }

    fn done(id: u32, name: &str, started: u32, completed: u32) -> Job {
        job(
            id,
            name,
            Some(at(started)),
            Some(at(completed)),
            "completed",
        )
    }

    fn at(minute: u32) -> String {
        format!("2026-09-27T10:{minute:02}:00Z")
    }

    fn named(jobs: &[Job]) -> Vec<Named<'_>> {
        jobs.iter()
            .enumerate()
            .map(|(order, job)| Named {
                job,
                name: job.name.clone(),
                order,
            })
            .collect()
    }

    fn labels(stages: &[Vec<Group>]) -> Vec<Vec<String>> {
        stages
            .iter()
            .map(|stage| {
                stage
                    .iter()
                    .map(|group| group.jobs[0].name.clone())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn reads_the_calling_workflow_off_a_jobs_name() {
        assert_eq!(
            split_name("Checks / Rust formatting"),
            (Some("Checks"), "Rust formatting")
        );
        assert_eq!(
            split_name("Build, verify, and publish"),
            (None, "Build, verify, and publish")
        );
        assert_eq!(split_name(" / odd"), (None, " / odd"));
    }

    #[test]
    fn does_not_split_inside_a_matrix_jobs_values() {
        assert_eq!(
            split_name("test (ubuntu / node 20)"),
            (None, "test (ubuntu / node 20)")
        );
        assert_eq!(
            split_name("Checks / test (a / b)"),
            (Some("Checks"), "test (a / b)")
        );
    }

    #[test]
    fn puts_a_called_workflows_jobs_in_one_box_and_what_waited_for_it_after() {
        let jobs = [
            done(1, "Checks / Frontend", 0, 3),
            done(2, "Checks / Rust", 0, 3),
            done(3, "Checks / macOS", 1, 7),
            done(4, "Build, verify, and publish", 8, 28),
        ];
        let stages = stages_of(&named(&jobs));
        assert_eq!(stages.len(), 2);
        assert_eq!(stages[0].len(), 1);
        assert_eq!(stages[0][0].key, "Checks");
        assert_eq!(stages[0][0].jobs.len(), 3);
        assert_eq!(stages[1][0].jobs[0].name, "Build, verify, and publish");
    }

    #[test]
    fn keeps_jobs_that_overlapped_side_by_side() {
        let jobs = [done(1, "lint", 0, 2), done(2, "test", 1, 5)];
        let stages = stages_of(&named(&jobs));
        assert_eq!(stages.len(), 1);
        assert_eq!(stages[0].len(), 2);
    }

    #[test]
    fn never_starts_a_stage_after_one_that_is_still_going() {
        let jobs = [
            job(1, "build", Some(at(0)), None, "in_progress"),
            job(2, "late", Some(at(9)), None, "in_progress"),
        ];
        assert_eq!(stages_of(&named(&jobs)).len(), 1);
    }

    #[test]
    fn puts_jobs_that_have_not_started_at_the_end() {
        let jobs = [
            job(1, "deploy", None, None, "queued"),
            done(2, "build", 0, 2),
        ];
        assert_eq!(labels(&stages_of(&named(&jobs))), [["build"], ["deploy"]]);
    }

    #[test]
    fn has_nothing_to_draw_for_a_run_with_no_jobs() {
        assert!(stages_of(&[]).is_empty());
    }

    #[test]
    fn counts_a_job_with_an_unreadable_start_as_not_started() {
        let jobs = [
            job(1, "odd", Some("yesterday".to_owned()), None, "queued"),
            done(2, "build", 0, 2),
        ];
        assert_eq!(labels(&stages_of(&named(&jobs))), [["build"], ["odd"]]);
    }

    #[test]
    fn keeps_jobs_that_started_together_in_the_order_they_were_listed() {
        let jobs = [done(1, "second", 0, 1), done(2, "first", 0, 1)];
        assert_eq!(labels(&stages_of(&named(&jobs))), [["second", "first"]]);
    }

    #[test]
    fn does_not_guess_what_waited_on_a_finished_job_whose_end_is_unknown() {
        let jobs = [
            job(1, "build", Some(at(0)), None, "completed"),
            done(2, "deploy", 5, 6),
        ];
        assert_eq!(stages_of(&named(&jobs)).len(), 1);
    }

    const RELEASE: &str = "jobs:
  approve:
    name: Approve the stable release
    if: ${{ !contains(github.ref_name, '-nightly.') }}
  pushed-checks:
    name: Checks already run on this commit
  checks:
    name: Checks
    needs: pushed-checks # only when the push's own run did not pass
    uses: ./.github/workflows/ci.yml
  build:
    name: Build and verify
    needs: approve
  build-linux:
    name: Build Linux packages
    needs: approve
  publish:
    name: Publish
    needs: [build, build-linux, pushed-checks, checks]
";
    const CI: &str = "jobs:
  changes:
    name: What changed
  frontend:
    name: Frontend quality, build, and tests
  rust:
    name: Rust formatting, lint, and tests
    needs: changes
  linux:
    name: Linux compile check and core tests
    needs:
      - changes
";

    fn files() -> WorkflowFiles {
        HashMap::from([
            (
                ".github/workflows/release.yml".to_owned(),
                read_jobs(RELEASE),
            ),
            (".github/workflows/ci.yml".to_owned(), read_jobs(CI)),
        ])
    }

    #[derive(Debug, PartialEq)]
    enum Shape {
        One(String),
        Many(Vec<String>),
    }

    fn shape(graph: &JobGraph) -> Vec<Vec<Shape>> {
        graph
            .columns
            .iter()
            .map(|column| {
                column
                    .iter()
                    .map(|drawn| match drawn.jobs.as_slice() {
                        [only] => Shape::One(only.label.clone()),
                        many => Shape::Many(many.iter().map(|each| each.label.clone()).collect()),
                    })
                    .collect()
            })
            .collect()
    }

    fn one(label: &str) -> Shape {
        Shape::One(label.to_owned())
    }

    fn many(labels: &[&str]) -> Shape {
        Shape::Many(labels.iter().map(|label| (*label).to_owned()).collect())
    }

    fn skipped(mut job: Job) -> Job {
        job.conclusion = Some("skipped".to_owned());
        job.outcome = Outcome::Skipped;
        job
    }

    #[test]
    fn lays_out_the_release_run_of_v0_5_3_nightly_6_as_github_drew_it() {
        let jobs = [
            done(1, "Checks already run on this commit", 0, 1),
            done(2, "Build and verify", 2, 30),
            done(3, "Build Linux packages", 2, 20),
            skipped(done(4, "Approve the stable release", 0, 0)),
            skipped(done(5, "Checks", 1, 1)),
            done(6, "Publish", 31, 32),
        ];
        let graph = graph_of(&jobs, &files(), Some(".github/workflows/release.yml"));
        assert_eq!(
            shape(&graph),
            [
                vec![
                    one("Checks already run on this commit"),
                    one("Approve the stable release")
                ],
                vec![
                    one("Checks / What changed"),
                    one("Checks / Frontend quality, build, and tests"),
                    many(&["Build and verify", "Build Linux packages"]),
                ],
                vec![many(&[
                    "Checks / Linux compile check and core tests",
                    "Checks / Rust formatting, lint, and tests",
                ])],
                vec![one("Publish")],
            ]
        );
        assert!(graph.columns[2][0]
            .jobs
            .iter()
            .all(|each| each.job.is_none() && each.skipped));
        let mut edges: Vec<String> = graph
            .edges
            .iter()
            .map(|edge| format!("{} > {}", edge.from, edge.to))
            .collect();
        edges.sort();
        let mut expected = [
            "approve > build",
            "build > publish",
            "checks/changes > checks/rust",
            "checks/frontend > publish",
            "checks/rust > publish",
            "pushed-checks > checks/changes",
            "pushed-checks > checks/frontend",
            "pushed-checks > publish",
        ];
        expected.sort();
        assert_eq!(edges, expected);
    }

    #[test]
    fn keeps_a_matrix_jobs_runs_in_one_box_under_its_name() {
        let matrix = HashMap::from([(
            "m.yml".to_owned(),
            read_jobs("jobs:\n  host:\n    name: Build the core (${{ matrix.arch }})\n"),
        )]);
        let jobs = [
            done(1, "Build the core (x86_64)", 0, 1),
            done(2, "Build the core (aarch64)", 0, 1),
        ];
        let graph = graph_of(&jobs, &matrix, Some("m.yml"));
        assert_eq!(graph.columns[0][0].title.as_deref(), Some("Build the core"));
        let labels: Vec<&str> = graph.columns[0][0]
            .jobs
            .iter()
            .map(|each| each.label.as_str())
            .collect();
        assert_eq!(labels, ["x86_64", "aarch64"]);
    }

    #[test]
    fn reads_the_stages_off_the_clock_when_a_job_is_not_in_the_file() {
        let jobs = [done(1, "Something else", 0, 1)];
        let graph = graph_of(&jobs, &files(), Some(".github/workflows/release.yml"));
        assert_eq!(shape(&graph), [vec![one("Something else")]]);
    }

    #[test]
    fn takes_off_the_runners_section_markers_and_marks_where_a_section_opens() {
        let raw = |number: u64, text: &str| LogLine {
            number,
            timestamp: None,
            text: text.to_owned(),
        };
        let log = JobLog {
            lines: vec![
                raw(1, "##[group]Run npm test"),
                raw(2, "npm test"),
                raw(3, "shell: /bin/bash -e {0}"),
                raw(4, "##[endgroup]"),
                raw(5, "error one"),
            ],
            expired: false,
            truncated: false,
        };
        let opened = log_of(log, &[]);
        let texts: Vec<&str> = opened.lines.iter().map(|line| line.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "Run npm test",
                "npm test",
                "shell: /bin/bash -e {0}",
                "",
                "error one"
            ]
        );
        let marks: Vec<Option<LogMark>> = opened.lines.iter().map(|line| line.mark).collect();
        assert_eq!(
            marks,
            [
                Some(LogMark::Group),
                None,
                None,
                Some(LogMark::Endgroup),
                None
            ]
        );
        let script: Vec<bool> = opened.lines.iter().map(|line| line.script).collect();
        assert_eq!(script, [false, true, false, false, false]);
    }

    fn line(number: u64, second: u32) -> LogLine {
        LogLine {
            number,
            timestamp: Some(format!("2026-09-27T10:00:{second:02}.5000000Z")),
            text: format!("line {number}"),
        }
    }

    fn step(number: u64, second: Option<u32>) -> Step {
        Step {
            number,
            name: format!("step {number}"),
            status: "completed".to_owned(),
            conclusion: Some("success".to_owned()),
            outcome: Outcome::Success,
            started_at: second.map(|second| format!("2026-09-27T10:00:{second:02}Z")),
            completed_at: None,
        }
    }

    fn starts(lines: &[LogLine], steps: &[Step]) -> Vec<(u64, usize)> {
        let stamps: Vec<Option<f64>> = lines
            .iter()
            .map(|line| millis(line.timestamp.as_deref()))
            .collect();
        let mut found: Vec<(u64, usize)> = step_starts(&stamps, steps).into_iter().collect();
        found.sort();
        found
    }

    #[test]
    fn finds_the_first_line_each_step_wrote() {
        let lines = [line(1, 0), line(2, 1), line(3, 4), line(4, 4), line(5, 9)];
        let steps = [step(1, Some(0)), step(2, Some(4)), step(3, Some(9))];
        assert_eq!(starts(&lines, &steps), [(1, 0), (2, 2), (3, 4)]);
    }

    #[test]
    fn steps_over_lines_with_no_time_on_them() {
        let lines = [
            LogLine {
                number: 1,
                timestamp: None,
                text: "no clock".to_owned(),
            },
            line(2, 4),
        ];
        assert_eq!(starts(&lines, &[step(1, Some(4))]), [(1, 1)]);
    }

    #[test]
    fn skips_a_step_that_never_ran_and_one_that_wrote_nothing() {
        let steps = [step(1, None), step(2, Some(0)), step(3, Some(30))];
        assert_eq!(starts(&[line(1, 0)], &steps), [(2, 0)]);
    }
}
