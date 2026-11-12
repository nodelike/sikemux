//! The Places page: the project's production places with how much agents may do at each,
//! the places that aren't production, and what agents sent out that nothing could place.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, Context, Entity, InteractiveElement, IntoElement, ParentElement,
    SharedString, Styled,
};
use sikemux_core::server::call;
use sikemux_core::server::places::{AddPlaceParams, PlaceLevelParams, PlacesParams, SeenParams};
use sikemux_view::places::{Level, Place, PlaceKind, PlaceStats, Places, Recognise, Seen};

use super::said::{self, Action, Moment};
use super::PlaybookTab;
use crate::chat::link::service_mark;
use crate::icons::Icon;
use crate::settings::rows;
use crate::theme::{Tokens, SPACE_1H, SPACE_2};
use crate::toast;
use crate::ui::{self, select, Button, Field, Pick, Variant};
use crate::workbench::Workbench;

const LEVELS: [(Level, &str, &str, &str); 3] = [
    (
        Level::Yours,
        "yours",
        "Yours",
        "Agents ship there and report after",
    ),
    (
        Level::TellMe,
        "tellMe",
        "Tell me",
        "Agents ship, and tell you the moment they do",
    ),
    (
        Level::Mine,
        "mine",
        "Mine",
        "Agents prepare it, and you say yes on a call",
    ),
];

/// What a place the person adds is recognised by, as the form offers it.
const FACTS: [(&str, &str); 7] = [
    ("A website", "https://hooks.example.com"),
    ("A command", "pnpm deploy:prod"),
    ("A Kubernetes context", "prod-eu"),
    ("An AWS profile", "production"),
    ("A Terraform workspace", "prod"),
    ("A Rundeck job", "prod/deploy-*"),
    ("A package registry", "npm"),
];

/// The production places, which the tab's list counts.
pub(super) fn production(places: &Places) -> usize {
    places
        .places
        .iter()
        .filter(|place| place.production)
        .count()
}

pub(super) fn page(
    settings: &PlaybookTab,
    folder: &str,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> Vec<AnyElement> {
    let Some(playbook) = settings.playbook.as_ref() else {
        return vec![rows::section(
            "Production places",
            None,
            None,
            false,
            rows::note("Reading…", cx),
            cx,
        )];
    };
    let places = &playbook.places;
    let (production, other): (Vec<&Place>, Vec<&Place>) =
        places.places.iter().partition(|place| place.production);
    let confirmed = places.confirmed;
    let mut listed: Vec<AnyElement> = match production.is_empty() {
        true => vec![rows::note(
            "None found. Tell any agent where the project ships, or add it here.",
            cx,
        )],
        false => (production.iter())
            .map(|place| place_row(place, confirmed.is_some(), folder, this, cx))
            .collect(),
    };
    if settings.adding.is_some() {
        listed.push(add_form(settings, folder, this, cx));
    }
    let (meta, sub) = match confirmed {
        Some(at) => (
            format!(
                "{} · you confirmed the list on {}",
                production.len(),
                said::day(at)
            ),
            "Agents run everything else. A change to one of these waits for your yes on a call, unless its level says otherwise.".to_owned(),
        ),
        None => (
            "found by Sikemux, not confirmed yet".to_owned(),
            unconfirmed_words(&production),
        ),
    };
    if confirmed.is_none() {
        listed.push(confirm_foot(settings.adding.is_some(), folder, this, cx));
    }
    let add = (confirmed.is_some() && settings.adding.is_none())
        .then(|| add_button(folder, Variant::Ghost, this).into_any_element());
    let mut page = vec![rows::section_with(
        "Production places",
        Some(meta.into()),
        add,
        Some(sub.into()),
        false,
        div().children(listed),
        cx,
    )];
    if !other.is_empty() {
        page.push(rows::section(
            "Other places",
            None,
            None,
            false,
            div().children(other.iter().map(|place| other_row(place, cx))),
            cx,
        ));
    }
    page.push(seen_section(places, folder, this, cx));
    page
}

/// What the list was read from, and that it waits until the person confirms it.
fn unconfirmed_words(production: &[&Place]) -> String {
    let mut sources: Vec<&str> = Vec::new();
    for found in production
        .iter()
        .filter_map(|place| place.found_in.as_deref())
    {
        if !sources.contains(&found) {
            sources.push(found);
        }
    }
    let read = match sources.split_last() {
        None => String::new(),
        Some((last, [])) => format!("Read from {last}. "),
        Some((last, rest)) => format!("Read from {} and {last}. ", rest.join(", ")),
    };
    let which = match production.len() {
        0 => {
            return "Until you confirm, a change to any place found waits for your yes.".to_owned()
        }
        1 => "it",
        2 => "either",
        _ => "any of them",
    };
    format!("{read}Until you confirm, a change to {which} waits for your yes.")
}

fn place_row(
    place: &Place,
    confirmed: bool,
    folder: &str,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let word = level_word(place.level);
    let under = match &place.said_by {
        Some(said_by) => {
            let whom = match (&said_by.with, place.said_from.as_deref()) {
                (Some(with), _) => format!(" in chat with {}", said::at_name(with.name.as_deref())),
                (None, Some(from)) if from.starts_with("a ") => format!(" on {from}"),
                (None, Some(from)) => format!(" in {from}"),
                (None, None) => String::new(),
            };
            let undo = place.was.map(|was| {
                let (folder, id) = (folder.to_owned(), place.id.clone());
                Action::new("Undo", move |bench, cx| {
                    set_level(bench, &folder, &id, was, cx)
                })
            });
            said::line(
                format!("playbook-place-{}", place.id),
                format!(
                    "{word}, since you said so{whom} on {}",
                    said::day(said_by.at)
                ),
                Moment::of(said_by),
                undo.into_iter().collect(),
                this,
                cx,
            )
        }
        None => about(found_words(place), cx),
    };
    rows::row_parts(
        Some(mark(place, cx)),
        place.name.clone(),
        Some(under),
        level_select(place, confirmed, folder, this, cx),
        cx,
    )
    .id(SharedString::from(format!(
        "playbook-place-row-{}",
        place.id
    )))
    .group(said::ROW)
    .into_any_element()
}

/// Where the place was found and how its calls went: "Found in vercel.json · 5 calls, all 5
/// answered as picked".
fn found_words(place: &Place) -> String {
    let found = place.found_in.as_ref().map_or_else(
        || "Added by you".to_owned(),
        |found| format!("Found in {found}"),
    );
    match calls_words(&place.stats) {
        Some(calls) => format!("{found} · {calls}"),
        None => found,
    }
}

fn calls_words(stats: &PlaceStats) -> Option<String> {
    let (asked, picked, overruled) = (stats.asked, stats.as_picked, stats.overruled);
    Some(match asked {
        0 => return None,
        1 if picked == 1 => "1 call, answered as picked".to_owned(),
        1 if overruled == 1 => "1 call, answered against the pick".to_owned(),
        1 => "1 call".to_owned(),
        _ if picked == asked => format!("{asked} calls, all {asked} answered as picked"),
        _ => format!("{asked} calls, {picked} answered as picked"),
    })
}

fn about(words: String, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    div()
        .text_size(crate::theme::ui_text(crate::theme::TEXT_2XS_H))
        .line_height(crate::theme::ui_text(crate::theme::TEXT_LG))
        .text_color(t.text_tertiary)
        .child(words)
        .into_any_element()
}

fn level_word(level: Level) -> &'static str {
    LEVELS
        .iter()
        .find(|(each, ..)| *each == level)
        .map_or("Mine", |(_, _, word, _)| word)
}

/// The place's level as a word, whose menu says what each level lets agents do.
fn level_select(
    place: &Place,
    confirmed: bool,
    folder: &str,
    this: &Entity<Workbench>,
    cx: &App,
) -> AnyElement {
    let shown = match confirmed {
        true => place.level,
        false => Level::Mine,
    };
    let chosen = LEVELS
        .iter()
        .find(|(level, ..)| *level == shown)
        .map(|(_, key, ..)| SharedString::from(*key));
    let (this, folder, id) = (this.clone(), folder.to_owned(), place.id.clone());
    select(
        SharedString::from(format!("playbook-place-level-{}", place.id)),
        level_word(shown),
        LEVELS
            .iter()
            .map(|(_, key, word, hint)| Pick::new(*key, *word).hint(*hint))
            .collect(),
        chosen,
        false,
        move |key, _, cx| {
            let Some((level, ..)) = LEVELS.iter().find(|(_, each, ..)| *each == key.as_ref())
            else {
                return;
            };
            let (folder, id, level) = (folder.clone(), id.clone(), *level);
            this.update(cx, |bench, cx| set_level(bench, &folder, &id, level, cx));
        },
        cx,
    )
}

fn set_level(
    bench: &mut Workbench,
    folder: &str,
    place: &str,
    level: Level,
    cx: &mut Context<Workbench>,
) {
    let asked = bench
        .target()
        .request_typed::<call::SetPlaceLevel>(PlaceLevelParams {
            project: folder.to_owned(),
            place: place.to_owned(),
            level,
        });
    let folder = folder.to_owned();
    bench.report(
        asked,
        "The level was not changed",
        move |bench, _, cx| bench.reload_playbook(&folder, cx),
        cx,
    );
}

fn other_row(place: &Place, cx: &mut Context<Workbench>) -> AnyElement {
    let words = match &place.found_in {
        Some(found) => {
            format!("Not production · found in {found} · agents deploy here without asking")
        }
        None => "Not production · agents deploy here without asking".to_owned(),
    };
    rows::row_parts(
        Some(mark(place, cx)),
        place.name.clone(),
        Some(about(words, cx)),
        div()
            .text_size(crate::theme::ui_text(crate::theme::TEXT_XS))
            .text_color(Tokens::get(cx).text_primary)
            .child("Yours"),
        cx,
    )
    .into_any_element()
}

/// The place's mark: its provider's own where it has one, else a glyph for what it is.
fn mark(place: &Place, cx: &App) -> AnyElement {
    let tint = Tokens::get(cx).text_secondary;
    let bundled = |name: &str| crate::file_icons::named(name, cx);
    let tool = place.name.rsplit_once(" on ").map(|(_, tool)| tool);
    let icon = match place.kind {
        PlaceKind::Vercel => Icon::Vercel,
        PlaceKind::Github => Icon::Github,
        PlaceKind::Aws => Icon::Aws,
        PlaceKind::Rundeck => Icon::Rundeck,
        PlaceKind::Database => Icon::Database,
        PlaceKind::Command => Icon::Terminal,
        PlaceKind::Kubernetes => return picture(bundled("kubernetes"), Icon::Server, tint),
        PlaceKind::Terraform => return picture(bundled("terraform"), Icon::Server, tint),
        PlaceKind::Site => site_mark(&place.name),
        PlaceKind::Connector => connector_mark(&place.name),
        PlaceKind::Other => {
            let file = match tool {
                Some("Heroku") => "heroku",
                Some("Firebase") => "firebase",
                Some("Netlify") => "netlify",
                Some("Supabase") => "supabase",
                Some("Cloudflare" | "Cloudflare Workers" | "Wrangler") => "wrangler",
                Some("Serverless") => "serverless",
                _ if place.name.contains("npm") => "npm",
                _ => return ui::picture(Icon::Server, 14., tint),
            };
            return picture(bundled(file), Icon::Server, tint);
        }
    };
    ui::picture(icon, 14., tint)
}

fn picture(
    image: Option<std::sync::Arc<gpui_kit::Image>>,
    instead: Icon,
    tint: gpui_kit::Hsla,
) -> AnyElement {
    match image {
        Some(image) => gpui_kit::img(image)
            .size(px(14.))
            .flex_none()
            .into_any_element(),
        None => ui::picture(instead, 14., tint),
    }
}

/// A site's service mark by its address, or the globe.
fn site_mark(address: &str) -> Icon {
    let href = match address.contains("://") {
        true => address.to_owned(),
        false => format!(
            "https://{}",
            address.split([',', ' ']).next().unwrap_or(address)
        ),
    };
    service_mark(&href).unwrap_or(Icon::Globe)
}

fn connector_mark(name: &str) -> Icon {
    let name = name.to_lowercase();
    [
        ("jira", Icon::Jira),
        ("slack", Icon::Slack),
        ("linear", Icon::Linear),
        ("github", Icon::Github),
        ("gitlab", Icon::Gitlab),
        ("forgejo", Icon::Forgejo),
        ("gitea", Icon::Gitea),
        ("bitbucket", Icon::Bitbucket),
        ("vercel", Icon::Vercel),
        ("rundeck", Icon::Rundeck),
        ("aws", Icon::Aws),
        ("postgres", Icon::Postgresql),
        ("mysql", Icon::Mysql),
    ]
    .into_iter()
    .find(|(word, _)| name.contains(word))
    .map_or(Icon::Plug, |(_, icon)| icon)
}

fn add_button(folder: &str, variant: Variant, this: &Entity<Workbench>) -> Button {
    let (this, folder) = (this.clone(), folder.to_owned());
    Button::new("playbook-place-add", "Add a place", variant).on_click(move |_, window, cx| {
        this.update(cx, |bench, cx| {
            if let Some(settings) = bench.settings_of(&folder) {
                settings.adding = Some(0);
                for input in [settings.place_name.clone(), settings.place_fact.clone()] {
                    input.update(cx, |input, cx| input.set_value("", window, cx));
                }
            }
            bench.parts.redraw_stage(cx);
        })
    })
}

/// The last row before the person confirms: where else to add a place, and Looks right.
fn confirm_foot(
    adding: bool,
    folder: &str,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let (confirm, confirm_folder) = (this.clone(), folder.to_owned());
    let looks_right = Button::new("playbook-places-confirm", "Looks right", Variant::Primary)
        .on_click(move |_, _, cx| {
            confirm.update(cx, |bench, cx| {
                let asked = bench
                    .target()
                    .request_typed::<call::ConfirmPlaces>(PlacesParams {
                        project: confirm_folder.clone(),
                    });
                let folder = confirm_folder.clone();
                bench.report(
                    asked,
                    "The list was not confirmed",
                    move |bench, _, cx| {
                        bench.notify(toast::success("Production places confirmed"), cx);
                        bench.reload_playbook(&folder, cx);
                    },
                    cx,
                );
            })
        });
    div()
        .flex()
        .items_center()
        .gap(SPACE_2)
        .px(crate::theme::SPACE_4)
        .py(px(11.))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(crate::theme::ui_text(crate::theme::TEXT_2XS_H))
                .text_color(t.text_tertiary)
                .child("Something missing? Tell any agent, or add it here."),
        )
        .when(!adding, |foot| {
            foot.child(add_button(folder, Variant::Ghost, this))
        })
        .child(looks_right)
        .into_any_element()
}

/// The person names a place and what it is recognised by.
fn add_form(
    settings: &PlaybookTab,
    folder: &str,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let kind = settings.adding.unwrap_or(0);
    let (label, _) = FACTS.get(kind).copied().unwrap_or(FACTS[0]);
    let picking = {
        let (this, folder) = (this.clone(), folder.to_owned());
        select(
            "playbook-place-fact",
            label,
            (FACTS.iter().enumerate())
                .map(|(at, (label, example))| Pick::new(at.to_string(), *label).hint(*example))
                .collect(),
            Some(kind.to_string().into()),
            false,
            move |key, _, cx| {
                let Ok(at) = key.parse::<usize>() else {
                    return;
                };
                let folder = folder.clone();
                this.update(cx, |bench, cx| {
                    if let Some(settings) = bench.settings_of(&folder) {
                        settings.adding = Some(at);
                    }
                    bench.parts.redraw_stage(cx);
                });
            },
            cx,
        )
    };
    let (save, save_folder) = (this.clone(), folder.to_owned());
    let add =
        Button::new("playbook-place-save", "Add", Variant::Primary).on_click(move |_, _, cx| {
            let folder = save_folder.clone();
            save.update(cx, |bench, cx| add_place(bench, &folder, cx))
        });
    let (cancel, cancel_folder) = (this.clone(), folder.to_owned());
    let close =
        Button::new("playbook-place-cancel", "Cancel", Variant::Ghost).on_click(move |_, _, cx| {
            let folder = cancel_folder.clone();
            cancel.update(cx, |bench, cx| {
                if let Some(settings) = bench.settings_of(&folder) {
                    settings.adding = None;
                }
                bench.parts.redraw_stage(cx);
            })
        });
    div()
        .flex()
        .flex_col()
        .gap(SPACE_2)
        .px(crate::theme::SPACE_4)
        .py(px(11.))
        .border_b_1()
        .border_color(Tokens::get(cx).line_soft)
        .child(Field::new(&settings.place_name))
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_2)
                .child(div().flex_none().child(picking))
                .child(
                    div()
                        .flex_1()
                        .child(Field::new(&settings.place_fact).mono()),
                ),
        )
        .child(
            div()
                .flex()
                .justify_end()
                .gap(SPACE_1H)
                .child(close)
                .child(add),
        )
        .into_any_element()
}

fn add_place(bench: &mut Workbench, folder: &str, cx: &mut Context<Workbench>) {
    let Some(settings) = bench.project_settings.get(folder) else {
        return;
    };
    let name = settings.place_name.read(cx).value().trim().to_owned();
    let value = settings.place_fact.read(cx).value().trim().to_owned();
    let fact = match settings.adding.unwrap_or(0) {
        0 => Recognise::Site { origin: value },
        1 => Recognise::Command { words: value },
        2 => Recognise::KubeContext { name: value },
        3 => Recognise::AwsProfile { name: value },
        4 => Recognise::TerraformWorkspace { name: value },
        5 => Recognise::RundeckJob { path: value },
        _ => Recognise::Package { registry: value },
    };
    let asked = bench
        .target()
        .request_typed::<call::AddPlace>(AddPlaceParams {
            project: folder.to_owned(),
            name,
            production: true,
            recognise: vec![fact],
        });
    let folder = folder.to_owned();
    bench.report(
        asked,
        "The place was not added",
        move |bench, _, cx| {
            if let Some(settings) = bench.settings_of(&folder) {
                settings.adding = None;
            }
            bench.reload_playbook(&folder, cx);
        },
        cx,
    );
}

fn seen_section(
    places: &Places,
    folder: &str,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let (meta, sub) = match places.seen.len() {
        0 => (
            "none yet".to_owned(),
            "What agents send out that nothing can place shows here after it runs.",
        ),
        count => (
            count.to_string(),
            "Agents sent these out and nothing could place them, so they ran. Each is listed once.",
        ),
    };
    let more = (places.more_seen > 0).then(|| {
        rows::note(
            format!(
                "And {} more, not listed while the list is full. Count some as production to make room.",
                places.more_seen
            ),
            cx,
        )
    });
    rows::section(
        "Seen, not gated",
        Some(meta.into()),
        Some(sub.into()),
        false,
        div()
            .children(
                places
                    .seen
                    .iter()
                    .map(|seen| seen_row(seen, folder, this, cx)),
            )
            .children(more),
        cx,
    )
}

fn seen_row(
    seen: &Seen,
    folder: &str,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let label = match &seen.from {
        Some(from) => format!("{}, from {from}", seen.what),
        None => seen.what.clone(),
    };
    let who = said::at_name(seen.last_name.as_deref());
    let when = ui::time::ago(seen.last_at);
    let words = match seen.times {
        1 => format!("Ran once · {who}, {when}"),
        times => format!("Ran {times} times · last by {who}, {when}"),
    };
    let tint = Tokens::get(cx).text_secondary;
    let icon = match seen.what.contains(' ') {
        true => Icon::Terminal,
        false => site_mark(&seen.what),
    };
    let (this, folder, id) = (this.clone(), folder.to_owned(), seen.id.clone());
    let count = Button::new(
        SharedString::from(format!("playbook-seen-{}", seen.id)),
        "Count as production",
        Variant::Outline,
    )
    .on_click(move |_, _, cx| {
        let (folder, id) = (folder.clone(), id.clone());
        this.update(cx, |bench, cx| {
            let asked = bench
                .target()
                .request_typed::<call::CountAsProduction>(SeenParams {
                    project: folder.clone(),
                    seen: id,
                });
            bench.report(
                asked,
                "It was not counted as production",
                move |bench, _, cx| bench.reload_playbook(&folder, cx),
                cx,
            );
        })
    });
    rows::row_parts(
        Some(ui::picture(icon, 14., tint)),
        label,
        Some(about(words, cx)),
        count,
        cx,
    )
    .into_any_element()
}
