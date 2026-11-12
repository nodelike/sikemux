//! The Rules page: the rules kept from the person's own words, and apart from them what agents
//! noted while working.

use gpui_kit::{
    AnyElement, Context, Entity, InteractiveElement, IntoElement, ParentElement, SharedString,
};
use sikemux_core::server::call;
use sikemux_core::server::memory::{MemoryAddParams, MemoryUpdateParams, NewMemory};
use sikemux_view::chat::RuleChange;
use sikemux_view::playbook::{Entry, Playbook};

use super::said::{self, Action, Moment};
use crate::core::Target;
use crate::settings::rows;
use crate::toast;
use crate::workbench::Workbench;

pub(super) fn page(
    playbook: Option<&Playbook>,
    folder: &str,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> Vec<AnyElement> {
    let Some(playbook) = playbook else {
        return vec![rows::section(
            t!("Your rules"),
            None,
            None,
            false,
            rows::note(t!("Reading…"), cx),
            cx,
        )];
    };
    let yours = match playbook.rules.is_empty() {
        true => vec![rows::note(
            t!("None yet. Tell any agent how you want things done here, and it keeps it as your rule."),
            cx,
        )],
        false => (playbook.rules.iter())
            .map(|rule| row(rule, n!("Undo"), folder, this, cx))
            .collect(),
    };
    let noted = match playbook.notes.is_empty() {
        true => vec![rows::note(t!("Nothing yet."), cx)],
        false => (playbook.notes.iter())
            .map(|note| row(note, n!("Remove"), folder, this, cx))
            .collect(),
    };
    let count = |n: usize| (n > 0).then(|| SharedString::from(n.to_string()));
    vec![
        rows::section(
            t!("Your rules"),
            count(playbook.rules.len()),
            Some(t!("Kept from your own words when you said them. Only you change these.").into()),
            false,
            gpui_kit::div().children(yours),
            cx,
        ),
        rows::section(
            t!("Noted by agents"),
            count(playbook.notes.len()),
            Some(
                t!("What agents noticed while working. It guides them, and never widens what they may do.")
                    .into(),
            ),
            false,
            gpui_kit::div().children(noted),
            cx,
        ),
    ]
}

fn row(
    entry: &Entry,
    undo: &'static str,
    folder: &str,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let mut words = format!(
        "{} · {}",
        said::who(&entry.learned),
        said::day(entry.learned.at)
    );
    if let Some(above) = &entry.folder {
        words.push_str(" · ");
        words.push_str(&t!(
            "for every project in {folder}",
            folder = crate::ui::path::tilde(above, None)
        ));
    }
    if entry.stale {
        words.push_str(" · ");
        words.push_str(t!("may no longer hold"));
    }
    let (id, folder) = (entry.id, folder.to_owned());
    let retire = Action::new(crate::i18n::tr(undo), move |bench, cx| {
        retire(bench, &folder, id, cx)
    });
    let line = said::line(
        format!("playbook-rule-{id}"),
        words,
        match entry.learned.agent {
            None => Moment::of(&entry.learned),
            Some(_) => None,
        },
        vec![if undo == "Undo" {
            retire.on_hover()
        } else {
            retire
        }],
        this,
        cx,
    );
    rows::row_parts(None, entry.text.clone(), Some(line), gpui_kit::div(), cx)
        .id(SharedString::from(format!("playbook-entry-{id}")))
        .group(said::ROW)
        .into_any_element()
}

pub(crate) fn retire(bench: &mut Workbench, folder: &str, id: u64, cx: &mut Context<Workbench>) {
    let asked = bench
        .target()
        .request_typed::<call::MemoryUpdate>(MemoryUpdateParams {
            project: folder.to_owned(),
            id,
            text: None,
            wrong: None,
            retire: true,
            about: None,
            check: None,
            drop_check: false,
            goal: None,
        });
    bench.report(
        asked,
        t!("It was not removed"),
        |bench, _, cx| bench.notify(toast::success(t!("Removed from the Playbook")), cx),
        cx,
    );
}

/// Brings back a rule of the person's that an agent took off the list or reworded, on the host
/// whose chat said so: kept again in its own words, which brings the set-aside rule back whole,
/// or put back to what it said before.
pub(crate) fn restore(
    bench: &mut Workbench,
    target: &Target,
    line: &RuleChange,
    cx: &mut Context<Workbench>,
) {
    let failed = t!("It was not brought back");
    let back = |bench: &mut Workbench, _, cx: &mut Context<Workbench>| {
        bench.notify(toast::success(t!("Back in the Playbook")), cx)
    };
    match &line.was {
        Some(was) => {
            let asked = target.request_typed::<call::MemoryUpdate>(MemoryUpdateParams {
                project: line.project.clone(),
                id: line.rule,
                text: Some(was.clone()),
                wrong: None,
                retire: false,
                about: None,
                check: None,
                drop_check: false,
                goal: None,
            });
            bench.report(asked, failed, back, cx);
        }
        None => {
            let asked = target.request_typed::<call::MemoryAdd>(MemoryAddParams {
                project: line.project.clone(),
                entry: NewMemory::Rule {
                    text: line.text.clone(),
                    goal: None,
                    sensitive: false,
                    folder: None,
                },
            });
            bench.report(asked, failed, back, cx);
        }
    }
}
