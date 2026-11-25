//! Reads a draft or sent message in order into plain words and the inline chips it wrote.

use crate::chat::{
    openings, reply_written, tracked_title, SentChip, SentChipKind, SentContext, SentSpan,
    TrackedKind, AGENT_URI, EARLIER_URI, WOKEN_URI,
};

const IMAGE_EXTENSIONS: [&str; 13] = [
    "png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "ico", "avif", "tif", "tiff", "heic", "heif",
];

fn is_image_path(path: &str) -> bool {
    path.rsplit_once('.').is_some_and(|(_, extension)| {
        IMAGE_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
    })
}

/// What a draft wrote into a text, as far as the text itself says.
#[derive(Default)]
pub struct SentNames<'a> {
    pub attachments: &'a [String],
    /// Which of `attachments` are folders.
    pub folders: &'a [String],
    pub context: &'a [SentContext],
    /// Agents the text may name by `@name`, as `(name, id)`, when no context item says so.
    pub agents: &'a [(String, String)],
    /// Commands the text may name by `/name`.
    pub commands: &'a [String],
}

/// `text` read in order, its words and the chips its draft wrote into it; empty when it holds
/// no chip. The core reads this once, so each client draws the same chips, whether or not the
/// agent a chip names is still open under that name.
pub fn sent_spans(text: &str, names: SentNames<'_>) -> Vec<SentSpan> {
    let found = sent_chips(text, &names);
    let mut spans = Vec::new();
    let mut rest = text;
    loop {
        let next = (found.iter())
            .filter_map(|(written, chip)| Some((rest.find(written.as_str())?, written, chip)))
            .min_by_key(|(at, written, _)| (*at, std::cmp::Reverse(written.len())));
        let Some((at, written, chip)) = next else {
            break;
        };
        let Some((before, from)) = rest.split_at_checked(at) else {
            break;
        };
        if !before.is_empty() {
            spans.push(SentSpan {
                text: before.to_owned(),
                chip: None,
            });
        }
        spans.push(SentSpan {
            text: written.clone(),
            chip: Some(chip.clone()),
        });
        rest = from.get(written.len()..).unwrap_or_default();
    }
    if spans.is_empty() {
        return spans;
    }
    if !rest.is_empty() {
        spans.push(SentSpan {
            text: rest.to_owned(),
            chip: None,
        });
    }
    spans
}

fn sent_chips(text: &str, names: &SentNames<'_>) -> Vec<(String, SentChip)> {
    let chip = |kind, label: &str, carried: Option<&str>| SentChip {
        kind,
        label: label.to_owned(),
        carried: carried.map(str::to_owned),
    };
    let mut found: Vec<(String, SentChip)> = Vec::new();
    for (name, id) in names.agents {
        let written = format!("@{name}");
        let carried = format!("{AGENT_URI}{id}");
        let agent = chip(SentChipKind::Agent, &written, Some(&carried));
        found.push((written, agent));
    }
    for name in names.commands {
        let written = format!("/{name}");
        let command = chip(SentChipKind::Command, &written, None);
        found.push((written, command));
    }
    for (index, path) in names.attachments.iter().enumerate() {
        if is_image_path(path) {
            let number = index + 1;
            let label = format!("Image #{number}");
            let tag = format!("[Image #{number}]");
            if text.contains(&tag) {
                found.push((tag, chip(SentChipKind::Image, &label, Some(path))));
                continue;
            }
        }
        let parts: Vec<&str> = path
            .split(['/', '\\'])
            .filter(|part| !part.is_empty())
            .collect();
        let tag = (0..parts.len())
            .filter_map(|from| parts.get(from..))
            .map(|tail| format!("+{}", tail.join("/")))
            .find(|tag| text.contains(tag.as_str()));
        if let Some(tag) = tag {
            let kind = match names.folders.contains(path) {
                true => SentChipKind::Folder,
                false => SentChipKind::File,
            };
            let file = chip(kind, &tag, Some(path));
            found.push((tag, file));
        }
    }
    for item in names.context {
        if item.uri.starts_with(AGENT_URI) || item.uri.starts_with(WOKEN_URI) {
            let written = item.title.split(' ').next().unwrap_or_default();
            if written.starts_with('@') {
                let agent = chip(SentChipKind::Agent, written, Some(&item.uri));
                found.push((written.to_owned(), agent));
            }
        } else if item.uri.starts_with(EARLIER_URI) && item.title.starts_with('@') {
            let earlier = chip(SentChipKind::Chat, &item.title, Some(&item.uri));
            found.push((item.title.clone(), earlier));
        } else if let Some(quote) = &item.quote {
            let opening = openings(quote)
                .rev()
                .find(|opening| text.contains(&reply_written(opening)));
            if let Some(opening) = opening {
                let reply = chip(SentChipKind::Reply, &opening, Some(&item.uri));
                found.push((reply_written(&opening), reply));
            }
        } else if let Some(tracked) = &item.tracked {
            let (sign, kind) = match tracked.kind {
                TrackedKind::Pull => ('!', SentChipKind::Pull),
                TrackedKind::Issue => ('#', SentChipKind::Issue),
            };
            let number = &tracked.number;
            let label = format!("#{number} {}", tracked_title(&item.title, number));
            let named = chip(kind, &label, Some(&item.uri));
            found.push((format!("{sign}{number}"), named));
        }
    }
    found.retain(|(written, _)| text.contains(written.as_str()));
    found
}
