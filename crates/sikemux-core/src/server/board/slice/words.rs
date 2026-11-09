//! What a pull request's description says of a slice: its promise, the one piece of
//! evidence its gate leads with, a line for each check on its latest commit and a link to
//! the slice, between two markers so a later push rewrites the same block.

use super::*;

/// Markdown renders these as nothing, on every code host.
const PROOF_START: &str = "[//]: # (sikemux proof)";
const PROOF_END: &str = "[//]: # (sikemux proof end)";

/// Most checks the block lists.
const PROOF_CHECKS: usize = 6;

impl Slice {
    /// The picture or recording the block leads with, by its evidence id and file.
    pub(crate) fn proof_picture(&self) -> Option<(u64, &str)> {
        match &self.leading()?.proof {
            Proof::Picture { path, .. } => Some((self.leading()?.id, path.as_str())),
            _ => None,
        }
    }

    /// The block in Markdown, showing the leading picture from `picture` where the code host
    /// keeps it, or `card` and `repo` link to the slice.
    pub(crate) fn proof_block(&self, card: u64, repo: &str, picture: Option<&str>) -> String {
        let mut text = format!("{PROOF_START}\n\n**Promise** · {}\n\n", self.promise);
        if let Some(lead) = self.leading() {
            text.push_str(&lead_words(lead, picture));
        }
        let checks = self.check_lines();
        if !checks.is_empty() {
            text.push_str(&checks.join("<br>\n"));
            text.push_str("\n\n");
        }
        text.push_str(&format!(
            "[Open slice #{card} in Sikemux](sikemux://slice/{card}?repo={repo})\n\n{PROOF_END}"
        ));
        text
    }

    /// Its pull request and the block its description holds, or would hold once one is
    /// open, with the picture by its file here until the code host keeps it.
    pub(crate) fn pull_view(&self, card: &super::super::Card) -> Value {
        let at = (self.pull.as_ref().map(|pull| &pull.at))
            .or_else(|| card.auto.as_ref().and_then(|run| run.pull.as_ref()));
        let repo = at.map_or_else(String::new, |at| format!("{}/{}", at.owner, at.name));
        let kept = (self.pull.as_ref())
            .and_then(|pull| pull.picture.as_ref())
            .filter(|kept| {
                self.proof_picture()
                    .is_some_and(|(id, _)| id == kept.evidence)
            });
        let picture = kept
            .map(|kept| kept.url.as_str())
            .or_else(|| self.proof_picture().map(|(_, path)| path));
        serde_json::json!({
            "url": at.map(|at| at.url.as_str()),
            "says": self.proof_block(card.id, &repo, picture),
        })
    }

    /// The newest verdict of each command run on its latest commit, as one line each.
    fn check_lines(&self) -> Vec<String> {
        let head = self.head();
        let mut said: Vec<(&str, Verdict)> = Vec::new();
        for evidence in self.evidence.iter().rev() {
            let (Proof::Run { command, .. }, Some(verdict)) = (&evidence.proof, evidence.verdict)
            else {
                continue;
            };
            let on_head = head.is_none() || evidence.commit.as_deref() == head;
            if on_head && !said.iter().any(|(known, _)| known == command) {
                said.push((command, verdict));
            }
        }
        said.reverse();
        let skipped = said.len().saturating_sub(PROOF_CHECKS);
        (said.into_iter().skip(skipped))
            .map(|(command, verdict)| {
                let mark = match verdict {
                    Verdict::Passed => "✅",
                    Verdict::Failed => "❌",
                };
                format!("{mark} `{command}`")
            })
            .collect()
    }
}

fn lead_words(lead: &Evidence, picture: Option<&str>) -> String {
    let on = lead
        .commit
        .as_deref()
        .map_or_else(String::new, |sha| format!(" on `{}`", short(sha)));
    match (&lead.proof, picture) {
        (
            Proof::Picture {
                seconds: None,
                width,
                ..
            },
            Some(url),
        ) => {
            let at = width.map_or_else(String::new, |width| format!(" at {width} px"));
            format!("![Screenshot{at}]({url})\n<sub>Screenshot{at}{on}</sub>\n\n")
        }
        (
            Proof::Picture {
                seconds: Some(seconds),
                ..
            },
            Some(url),
        ) => {
            format!("[▶ Recording, {seconds:.0} s]({url}){on}\n\n")
        }
        (Proof::Numbers { measures }, _) => {
            let said: Vec<String> = (measures.iter())
                .map(|measure| match &measure.before {
                    Some(before) => format!("{} {before} → {}", measure.label, measure.after),
                    None => format!("{} {}", measure.label, measure.after),
                })
                .collect();
            format!("{}{on}\n\n", said.join(" · "))
        }
        (
            Proof::Diff {
                files,
                added,
                removed,
                ..
            },
            _,
        ) => {
            let count = files.len();
            let files = if count == 1 { "file" } else { "files" };
            format!("+{added} −{removed} in {count} {files}{on}\n\n")
        }
        _ => String::new(),
    }
}

/// `body` with `block` in place of the one it holds, or after what it says when it holds none.
pub(crate) fn with_proof(body: &str, block: &str) -> String {
    let held = body.find(PROOF_START).and_then(|start| {
        let end = start + body[start..].find(PROOF_END)? + PROOF_END.len();
        Some((start, end))
    });
    match held {
        Some((start, end)) => format!("{}{block}{}", &body[..start], &body[end..]),
        None if body.trim().is_empty() => block.to_owned(),
        None => format!("{}\n\n{block}", body.trim_end()),
    }
}

pub(crate) fn proof_words(proof: &Proof) -> &'static str {
    match proof {
        Proof::Picture {
            seconds: Some(_), ..
        } => "recording",
        Proof::Picture { .. } => "screenshot",
        Proof::Diff { .. } => "change",
        Proof::Numbers { .. } => "numbers",
        Proof::Run { .. } => "check run",
        Proof::Timeline { .. } => "timeline",
    }
}
