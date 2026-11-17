//! The Rules page: the rules kept from the person's own words, and apart from them what agents
//! noted while working.

use gpui_kit::{
    AnyElement, Context, Entity, InteractiveElement, IntoElement, ParentElement, SharedString,
    Styled,
};
use sikemux_calls::call;
use sikemux_calls::memory::{MemoryAddParams, MemoryParams, MemoryUpdateParams, NewMemory};
use sikemux_view::chat::RuleChange;
use sikemux_view::memory::{MoveOffer, Store};
use sikemux_view::playbook::{Entry, Playbook};

use super::said::{self, Action, Moment};
use crate::core::Target;
use crate::icons::Icon;
use crate::settings::rows;
use crate::theme::{Tokens, SPACE_1, SPACE_2, SPACE_2H, TEXT_XS};
use crate::toast;
use crate::ui::{self, Button, Variant};
use crate::workbench::Workbench;

const RECENT_MS: u64 = 24 * 60 * 60 * 1000;

pub(super) fn page(
    playbook: Option<&Playbook>,
    folder: &str,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> Vec<AnyElement> {
    let Some(playbook) = playbook else {
        return vec![rows::section(
            t!("Yours"),
            None,
            None,
            false,
            rows::note(t!("Reading…"), cx),
            cx,
        )];
    };
    let name = crate::ui::path::file_name(std::path::Path::new(folder));
    let count = |n: usize| (n > 0).then(|| SharedString::from(n.to_string()));
    let group =
        |rules: Vec<&Entry>, empty: &str, cx: &mut Context<Workbench>| match rules.is_empty() {
            true => vec![rows::note(empty, cx)],
            false => (rules.into_iter())
                .map(|rule| row(rule, n!("Undo"), folder, &name, this, cx))
                .collect::<Vec<_>>(),
        };
    let (project, yours): (Vec<&Entry>, Vec<&Entry>) =
        (playbook.rules.iter()).partition(|rule| rule.store.is_project());
    let (yours_count, project_count) = (count(yours.len()), count(project.len()));
    let has_project_rules = !project.is_empty();
    let yours = group(
        yours,
        t!("None yet. Tell any agent how you like to work everywhere, and it keeps it as yours."),
        cx,
    );
    let project = group(
        project,
        t!("None yet. Tell any agent how you want things done here, and it keeps it for this project."),
        cx,
    );
    let noted = match playbook.notes.is_empty() {
        true => vec![rows::note(t!("Nothing yet."), cx)],
        false => (playbook.notes.iter())
            .map(|note| row(note, n!("Remove"), folder, &name, this, cx))
            .collect(),
    };
    vec![
        rows::section(
            t!("Yours"),
            yours_count,
            Some(t!("Kept from your own words. They follow you to every project.").into()),
            false,
            gpui_kit::div().children(yours),
            cx,
        ),
        rows::section_with(
            name.clone(),
            project_count,
            has_project_rules.then(|| project_menu(folder, this, cx)),
            Some(t!("Every agent on {project} follows these. Kept by Sikemux, never in the repository.", project = name).into()),
            false,
            gpui_kit::div().children(project),
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

fn project_menu(folder: &str, this: &Entity<Workbench>, cx: &mut Context<Workbench>) -> AnyElement {
    use gpui_kit::component::button::ButtonVariants;
    use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
    let t = Tokens::get(cx).clone();
    let (this, folder) = (this.downgrade(), folder.to_owned());
    gpui_kit::component::button::Button::new("playbook-rules-more")
        .child(ui::icon(Icon::More, 13.).text_color(t.text_secondary))
        .ghost()
        .compact()
        .rounded(crate::theme::radius_2())
        .dropdown_menu_with_anchor(
            gpui_kit::Anchor::TopRight,
            ui::menu(move |popup, _, _| {
                let (this, folder) = (this.clone(), folder.clone());
                popup.item(
                    PopupMenuItem::new(t!("Write into AGENTS.md"))
                        .icon(Icon::File)
                        .on_click(move |_, _, cx| {
                            let _ =
                                this.update(cx, |bench, cx| write_agents_file(bench, &folder, cx));
                        }),
                )
            }),
        )
        .into_any_element()
}

fn write_agents_file(bench: &mut Workbench, folder: &str, cx: &mut Context<Workbench>) {
    let asked = bench
        .target()
        .request_typed::<call::PromoteRules>(MemoryParams {
            project: folder.to_owned(),
        });
    bench.report(
        asked,
        t!("AGENTS.md was not written"),
        |bench, _, cx| {
            bench.notify(
                toast::success(t!("Written into AGENTS.md, uncommitted for you to review")),
                cx,
            )
        },
        cx,
    );
}

/// "just now", "2 min ago" or "3 h ago" for the last day, else the day.
fn when(at: u64) -> String {
    let minutes = crate::ui::time::now_ms().saturating_sub(at) / 60_000;
    match minutes {
        0 => t!("just now").to_owned(),
        1..60 => t!("{minutes} min ago", minutes = minutes),
        60..1440 => t!("{hours} h ago", hours = minutes / 60),
        _ => said::day(at),
    }
}

fn recent(at: u64) -> bool {
    crate::ui::time::now_ms().saturating_sub(at) < RECENT_MS
}

fn store_name(store: Store, project: &str) -> String {
    match store {
        Store::Personal => t!("yours").to_owned(),
        Store::Project => project.to_owned(),
    }
}

fn row(
    entry: &Entry,
    undo: &'static str,
    folder: &str,
    project: &str,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let (id, store) = (entry.id, entry.store);
    let moved = (entry.moved.as_ref()).filter(|moved| recent(moved.at));
    let new = entry.someone.is_some() && moved.is_none() && recent(entry.learned.at);
    let who = match &entry.someone {
        Some(someone) => someone.clone(),
        None => said::who(&entry.learned),
    };
    let mut words = Vec::new();
    if entry.sensitive {
        words.push(t!("Sensitive").to_owned());
    }
    match moved {
        Some(moved) => {
            words.push(said::who(moved));
            words.push(t!(
                "moved from {store} {when}",
                store = store_name(store.other(), project),
                when = when(moved.at)
            ));
        }
        None if new => {
            words.push(who);
            words.push(when(entry.learned.at));
            words.push(t!("new").to_owned());
        }
        None => {
            words.push(who);
            words.push(said::day(entry.learned.at));
        }
    }
    if entry.stale {
        words.push(t!("may no longer hold").to_owned());
    }
    let mut inline = Vec::new();
    if moved.is_some() {
        let folder = folder.to_owned();
        let to = store.other();
        inline.push(Action::new(t!("Undo"), move |bench, cx| {
            move_entry(bench, &folder, id, to, cx)
        }));
    } else if new {
        let folder = folder.to_owned();
        inline.push(Action::new(t!("Undo"), move |bench, cx| {
            retire(bench, &folder, id, cx)
        }));
    }
    let line = said::line(
        format!("playbook-rule-{id}"),
        words.join(" · "),
        match entry.learned.agent {
            None => Moment::of(&entry.learned),
            Some(_) => None,
        },
        inline,
        this,
        cx,
    );
    let line = match entry.sensitive {
        true => gpui_kit::div()
            .flex()
            .items_center()
            .gap(SPACE_1)
            .child(ui::icon(Icon::Lock, 10.).text_color(t.text_tertiary))
            .child(line)
            .into_any_element(),
        false => line,
    };
    let about = gpui_kit::div().flex().flex_col().child(line).children(
        entry
            .offer
            .as_ref()
            .map(|offer| offer_box(entry, offer, folder, project, this, cx)),
    );
    let mut hover = Vec::new();
    if moved.is_none() {
        let (folder, project, this) = (folder.to_owned(), project.to_owned(), this.clone());
        let to = store.other();
        hover.push(
            Button::new(
                SharedString::from(format!("playbook-move-{id}")),
                t!("Move to {store}", store = store_name(to, &project)),
                Variant::Outline,
            )
            .icon(Icon::Move)
            .on_click(move |_, _, cx| {
                this.update(cx, |bench, cx| move_entry(bench, &folder, id, to, cx))
            })
            .into_any_element(),
        );
    }
    if !new {
        let (folder, this) = (folder.to_owned(), this.clone());
        hover.push(
            Button::new(
                SharedString::from(format!("playbook-undo-{id}")),
                crate::i18n::tr(undo),
                Variant::Ghost,
            )
            .on_click(move |_, _, cx| this.update(cx, |bench, cx| retire(bench, &folder, id, cx)))
            .into_any_element(),
        );
    }
    let hover = gpui_kit::div()
        .flex()
        .gap(SPACE_1)
        .opacity(0.)
        .group_hover(said::ROW, |shown| shown.opacity(1.))
        .children(hover);
    rows::row_parts(
        None,
        entry.text.clone(),
        Some(about.into_any_element()),
        hover,
        cx,
    )
    .id(SharedString::from(format!("playbook-entry-{id}")))
    .group(said::ROW)
    .into_any_element()
}

/// The chief's offer under a rule to move it to the other store, with Not now.
fn offer_box(
    entry: &Entry,
    offer: &MoveOffer,
    folder: &str,
    project: &str,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let (id, to) = (entry.id, entry.store.other());
    let said = match offer {
        MoveOffer::AboutHere => t!(
            "Everyone's agents on {project} would follow this too.",
            project = project
        ),
        MoveOffer::SaidElsewhere { elsewhere } => t!(
            "You keep this on {elsewhere} too. As yours, it would follow you to every project.",
            elsewhere = elsewhere
        ),
    };
    let moving = (this.clone(), folder.to_owned());
    let declining = (this.clone(), folder.to_owned());
    gpui_kit::div()
        .mt(SPACE_2)
        .w_full()
        .px(SPACE_2H)
        .py(SPACE_2)
        .flex()
        .flex_col()
        .gap(SPACE_2)
        .rounded(crate::theme::radius_2())
        .border_1()
        .border_color(t.line_soft)
        .bg(t.surface_raised)
        .text_size(crate::theme::ui_text(TEXT_XS))
        .text_color(t.text_secondary)
        .child(
            gpui_kit::div()
                .w_full()
                .child(gpui_kit::div().text_color(t.text_primary).child("@Chief"))
                .child(said),
        )
        .child(
            gpui_kit::div()
                .flex()
                .items_center()
                .gap(SPACE_2)
                .child(
                    Button::new(
                        SharedString::from(format!("playbook-offer-{id}-move")),
                        t!("Move to {store}", store = store_name(to, project)),
                        Variant::Outline,
                    )
                    .icon(Icon::Move)
                    .on_click(move |_, _, cx| {
                        let (this, folder) = &moving;
                        this.update(cx, |bench, cx| move_entry(bench, folder, id, to, cx))
                    }),
                )
                .child(
                    Button::new(
                        SharedString::from(format!("playbook-offer-{id}-not-now")),
                        t!("Not now"),
                        Variant::Ghost,
                    )
                    .on_click(move |_, _, cx| {
                        let (this, folder) = &declining;
                        this.update(cx, |bench, cx| not_now(bench, folder, id, cx))
                    }),
                ),
        )
        .into_any_element()
}

fn update(project: &str, id: u64) -> MemoryUpdateParams {
    MemoryUpdateParams {
        project: project.to_owned(),
        id,
        text: None,
        wrong: None,
        retire: false,
        about: None,
        check: None,
        drop_check: false,
        goal: None,
        store: None,
        not_now: false,
    }
}

fn moved_toast(to: Store, folder: &str) -> String {
    match to {
        Store::Personal => t!("Moved to yours").to_owned(),
        Store::Project => t!(
            "Moved to {project}",
            project = crate::ui::path::file_name(std::path::Path::new(folder))
        ),
    }
}

fn move_entry(
    bench: &mut Workbench,
    folder: &str,
    id: u64,
    to: Store,
    cx: &mut Context<Workbench>,
) {
    let asked = bench
        .target()
        .request_typed::<call::MemoryUpdate>(MemoryUpdateParams {
            store: Some(to),
            ..update(folder, id)
        });
    let said = moved_toast(to, folder);
    bench.report(
        asked,
        t!("It was not moved"),
        move |bench, _, cx| bench.notify(toast::success(said), cx),
        cx,
    );
}

fn not_now(bench: &mut Workbench, folder: &str, id: u64, cx: &mut Context<Workbench>) {
    let asked = bench
        .target()
        .request_typed::<call::MemoryUpdate>(MemoryUpdateParams {
            not_now: true,
            ..update(folder, id)
        });
    bench.report(
        asked,
        t!("It was not changed"),
        |bench, _, cx| bench.notify(toast::success(t!("Kept where it is")), cx),
        cx,
    );
}

pub(crate) fn retire(bench: &mut Workbench, folder: &str, id: u64, cx: &mut Context<Workbench>) {
    let asked = bench
        .target()
        .request_typed::<call::MemoryUpdate>(MemoryUpdateParams {
            retire: true,
            ..update(folder, id)
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
    if line.moved {
        let asked = target.request_typed::<call::MemoryUpdate>(MemoryUpdateParams {
            store: Some(line.store.other()),
            ..update(&line.project, line.rule)
        });
        let back = moved_toast(line.store.other(), &line.project);
        bench.report(
            asked,
            t!("It was not moved back"),
            move |bench, _, cx| bench.notify(toast::success(back), cx),
            cx,
        );
        return;
    }
    let failed = t!("It was not brought back");
    let back = |bench: &mut Workbench, _, cx: &mut Context<Workbench>| {
        bench.notify(toast::success(t!("Back in the Playbook")), cx)
    };
    match &line.was {
        Some(was) => {
            let asked = target.request_typed::<call::MemoryUpdate>(MemoryUpdateParams {
                text: Some(was.clone()),
                ..update(&line.project, line.rule)
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
                    store: line.store,
                },
            });
            bench.report(asked, failed, back, cx);
        }
    }
}
