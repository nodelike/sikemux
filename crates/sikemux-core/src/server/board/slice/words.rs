//! What a pull request's body says of a slice.

use super::*;

/// Most pieces of evidence a pull request's description lists.
pub(crate) const PULL_EVIDENCE: usize = 12;

impl Slice {
    /// What a pull request opened for the slice's work says of its evidence, in
    /// Markdown: the newest pieces collected on its latest commit, the pictures and
    /// recordings by name and the host they are kept on, as no code host here takes
    /// uploads. None while it has none. What a check printed and where a file lies on
    /// the host stay out.
    pub(crate) fn pull_evidence(&self, host: &str) -> Option<String> {
        let head = self.head();
        let on_head = |evidence: &&Evidence| head.is_none() || evidence.commit.as_deref() == head;
        let mut shown: Vec<&Evidence> = (self.evidence.iter().rev())
            .filter(on_head)
            .take(PULL_EVIDENCE)
            .collect();
        if shown.is_empty() {
            return None;
        }
        shown.reverse();
        let at = head.map_or_else(String::new, |head| format!(" for `{}`", short(head)));
        let mut text = format!(
            "### Evidence\n\nCollected by Sikemux on {host}{at}, where its pictures and recordings are kept with the card.\n\n"
        );
        for evidence in shown {
            text.push_str(&format!("- {}\n", pull_line(evidence)));
        }
        Some(text)
    }
}

pub(crate) fn pull_line(evidence: &Evidence) -> String {
    let named = |path: &str| {
        Path::new(path).file_name().map_or_else(
            || path.to_owned(),
            |name| name.to_string_lossy().into_owned(),
        )
    };
    match &evidence.proof {
        Proof::Run { command, sha, .. } => {
            let went = match evidence.verdict {
                Some(Verdict::Passed) => "passed",
                Some(Verdict::Failed) => "failed",
                None => "gave no verdict",
            };
            let on = sha
                .as_deref()
                .map_or_else(String::new, |sha| format!(" on `{}`", short(sha)));
            format!("Check `{command}` {went}{on}")
        }
        Proof::Diff {
            base,
            head,
            files,
            added,
            removed,
        } => {
            let some: Vec<String> = files
                .iter()
                .take(3)
                .map(|file| format!("`{file}`"))
                .collect();
            let more = if files.len() > some.len() {
                " and more"
            } else {
                ""
            };
            format!(
                "Change `{}..{}`: +{added} −{removed} in {}{more}",
                short(base),
                short(head),
                some.join(", ")
            )
        }
        Proof::Picture {
            path,
            seconds: None,
            ..
        } => format!("Screenshot `{}`", named(path)),
        Proof::Picture {
            path,
            seconds: Some(seconds),
            ..
        } => format!("Recording `{}`, {seconds:.0} s", named(path)),
        Proof::Numbers { measures } => {
            let said: Vec<String> = (measures.iter())
                .map(|measure| match &measure.before {
                    Some(before) => format!("{} {before} → {}", measure.label, measure.after),
                    None => format!("{} {}", measure.label, measure.after),
                })
                .collect();
            format!("Numbers: {}", said.join("; "))
        }
        Proof::Timeline { moments } => format!("Timeline of {} events", moments.len()),
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
