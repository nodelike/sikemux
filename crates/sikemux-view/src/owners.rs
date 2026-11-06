//! Who wrote which lines: uncommitted ones, as the core works them out, and those of a commit.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Owners {
    pub authors: Vec<LineAuthor>,
    pub files: Vec<FileOwners>,
}

/// An agent, or the agents at work together when no one of them could have
/// made a change, named "Otter or Briar", with no agent id of its own.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LineAuthor {
    pub agent_id: String,
    pub name: String,
    pub cli: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The chat run of its latest edit.
    pub feed: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shared: Vec<String>,
    /// Whether the agent is still open here; lines of one closed since block
    /// nobody who names their file.
    #[serde(skip)]
    pub open: bool,
}

/// The changed lines of a file agents wrote, by its path in the checkout.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileOwners {
    pub path: String,
    pub lines: Vec<LineRun>,
    pub removed: Vec<LineRemoval>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<LineNote>,
}

/// What to say beside a changed line: who wrote it, said where its run of changed
/// lines starts and wherever the writer changes within it, and whose lines it
/// rewrote.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LineNote {
    pub line: u64,
    /// Whether the note names who wrote the line; `author` none when no agent did.
    pub named: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub over: Option<usize>,
}

/// `count` lines from `start` that `author` wrote last. `over` is the agent
/// whose lines it rewrote there.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LineRun {
    pub start: u64,
    pub count: u64,
    pub author: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub over: Option<usize>,
}

/// Lines `author` took out just before line `at`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LineRemoval {
    pub at: u64,
    pub author: usize,
}

/// Who wrote which lines of a commit, as its note gives each of the agents its
/// footer names, by name.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CommitLines {
    pub authors: Vec<LineNamed>,
    pub files: Vec<FileOwners>,
}

/// Agents by name; several who wrote the same lines read "Otter or Briar".
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LineNamed {
    pub name: String,
}

impl Owners {
    /// The authors with a line or a removal among the files, by number.
    pub fn writers(&self) -> impl Iterator<Item = (usize, &LineAuthor)> {
        let wrote: HashSet<usize> = self
            .files
            .iter()
            .flat_map(|file| {
                let lines = file.lines.iter().map(|run| run.author);
                lines.chain(file.removed.iter().map(|removal| removal.author))
            })
            .collect();
        self.authors
            .iter()
            .enumerate()
            .filter(move |(at, _)| wrote.contains(at))
    }
}

impl LineAuthor {
    pub fn agents(&self) -> impl Iterator<Item = &str> {
        let own = self.shared.is_empty().then_some(&self.agent_id);
        own.into_iter().chain(&self.shared).map(String::as_str)
    }
}

/// Asks who wrote each uncommitted line of the checkout at `dir`, or its staged half.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CheckoutParams {
    pub dir: PathBuf,
    #[serde(default)]
    pub staged: bool,
}

/// Runs of lines, first and last, by path.
pub type Wrote = BTreeMap<String, Vec<(u64, u64)>>;

/// An agent a commit's footer names, as it stands here now.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentOwner {
    pub name: String,
    pub cli: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The version of Sikemux the agent ran in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sikemux: Option<String>,
    /// The agent the footer names, which stands for the session until it is known.
    pub chat: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The agent that has its session open now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open: Option<String>,
    /// The CLI's own session, which resuming the agent needs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// The lines it wrote in the commit, as the commit's note says, when the
    /// commit holds lines of others too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<Wrote>,
}

/// Asks which agents each of `commits` has in its footer.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommitsParams {
    pub dir: PathBuf,
    pub commits: Vec<String>,
}

/// Asks who wrote each line `commit` changed.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommitParams {
    pub dir: PathBuf,
    pub commit: String,
}
