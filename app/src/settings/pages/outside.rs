//! Apps outside Sikemux that use its chats through MCP: the address to give them, how to add
//! it to Claude Code and Codex, and the apps let in, to remove.

use gpui_kit::{
    div, px, AnyElement, App, ClipboardItem, Entity, FontWeight, IntoElement, ParentElement,
    SharedString, Styled,
};
use sikemux_calls::call;
use sikemux_calls::outside::{OutsideChange, OutsideClient, Via};
use sikemux_view::prefs::TabRailPage;
use sikemux_wire::protocol::DeviceAccess;

use crate::icons::Icon;
use crate::settings::index::Page::Developer;
use crate::settings::index::{section, Entry};
use crate::settings::rows;
use crate::theme::{
    self, Tokens, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_4, TEXT_2XS, TEXT_SM, TEXT_XS,
};
use crate::ui::{self, Tone};
use crate::workbench::Workbench;

pub(in crate::settings) const ENTRIES: &[Entry] = &[
    section(
        Developer,
        "MCP server",
        "developer mcp server url address endpoint claude code codex cursor chatgpt outside apps agents oauth",
    ),
    section(
        Developer,
        "Add to an app",
        "developer claude mcp add codex mcp add login connect command",
    ),
    section(
        Developer,
        "Allowed apps",
        "developer apps signed in revoke remove access full watch read only",
    ),
];

fn copy(this: &Entity<Workbench>, id: &str, label: &'static str, text: String) -> ui::IconButton {
    let copying = this.clone();
    ui::IconButton::new(
        SharedString::from(format!("outside-copy-{id}")),
        Icon::Copy,
        label,
    )
    .on_click(move |_, _, cx| {
        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        copying.update(cx, |bench, cx| {
            bench.notify(crate::toast::success(t!("Copied")), cx)
        });
    })
}

fn about(client: &OutsideClient) -> SharedString {
    let access = match client.access {
        DeviceAccess::Full => t!("Full control"),
        DeviceAccess::Watch => t!("Watch only"),
    };
    let from = match (&client.via, &client.redirect) {
        (Via::Cli, _) => "sikemux chat".to_owned(),
        (Via::Mcp, Some(redirect)) => t!("MCP through {redirect}", redirect = redirect),
        (Via::Mcp, None) => "MCP".to_owned(),
    };
    let used = match client.last_used {
        Some(at) => t!("used {ago}", ago = crate::ui::time::ago(at)),
        None => t!("not used yet").to_owned(),
    };
    format!("{access} · {from} · {used}").into()
}

fn remove(client: &OutsideClient, this: &Entity<Workbench>) -> AnyElement {
    let (removing, id, name) = (this.clone(), client.id.clone(), client.name.clone());
    ui::Button::new(
        SharedString::from(format!("outside-remove-{}", client.id)),
        t!("Remove"),
        ui::Variant::Ghost,
    )
    .on_click(move |_, _, cx| {
        let id = id.clone();
        let name = name.clone();
        removing.update(cx, |bench, cx| {
            remove_app(bench, id);
            bench.notify(crate::toast::success(t!("{name} removed", name = name)), cx);
        })
    })
    .into_any_element()
}

fn remove_app(bench: &Workbench, id: String) {
    crate::core::Target::here(&bench.asks)
        .call_typed::<call::ChangeOutside>(OutsideChange { id, access: None });
}

pub fn render(
    bench: &Workbench,
    lit: &dyn Fn(&str) -> bool,
    this: &Entity<Workbench>,
    cx: &App,
) -> Vec<AnyElement> {
    let t = Tokens::get(cx).clone();
    let outside = bench.reads.outside.as_ref();
    let url = outside.and_then(|outside| outside.url.clone());
    let error = outside.and_then(|outside| outside.error.clone());
    let code_block = |text: String| {
        div()
            .flex_1()
            .min_w_0()
            .px(SPACE_2H)
            .py(SPACE_1H)
            .rounded(theme::radius_2())
            .bg(t.surface_sunken)
            .border(crate::theme::border())
            .border_color(t.border)
            .font_family(theme::mono_font())
            .text_size(theme::ui_text(TEXT_2XS))
            .line_height(theme::ui_text(17.))
            .text_color(t.text_primary)
            .whitespace_normal()
            .child(text)
    };
    let tile = div()
        .flex_none()
        .size(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme::radius_2())
        .bg(t.surface_raised)
        .border(crate::theme::border())
        .border_color(t.border)
        .child(ui::icon(Icon::Plug, 15.).text_color(t.text_primary));
    let (status_tone, status_text) = match (&url, &error) {
        (Some(_), _) => (Tone::Ok, t!("Running")),
        (None, Some(_)) => (Tone::Failed, t!("Stopped")),
        (None, None) => (Tone::Pending, t!("Starting…")),
    };
    let status_pill = div()
        .flex()
        .items_center()
        .gap(SPACE_1H)
        .child(ui::dot(status_tone, cx))
        .child(
            div()
                .text_size(theme::ui_text(TEXT_XS))
                .text_color(t.text_secondary)
                .child(status_text),
        );
    let endpoint = match (&url, &error) {
        (Some(url), _) => div()
            .px(SPACE_4)
            .py(SPACE_2H)
            .border_b(crate::theme::border())
            .border_color(t.line_soft)
            .flex()
            .items_start()
            .gap(SPACE_2)
            .child(
                div()
                    .w(px(116.))
                    .pt(SPACE_1H)
                    .flex_none()
                    .text_size(theme::ui_text(TEXT_XS))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(t.text_secondary)
                    .child(t!("Endpoint")),
            )
            .child(code_block(url.clone()))
            .child(copy(this, "url", t!("Copy endpoint"), url.clone()))
            .into_any_element(),
        (None, Some(error)) => {
            rows::note(t!("The server is not running: {error}", error = error), cx)
        }
        (None, None) => rows::note(t!("Starting…"), cx),
    };
    let server = div()
        .flex()
        .flex_col()
        .child(rows::row_parts(
            Some(tile.into_any_element()),
            div()
                .font_weight(FontWeight::MEDIUM)
                .child(t!("Let other apps use Sikemux’s tools")),
            Some(rows::about_line(
                t!("Only apps on this host can sign in to list, message and read your chats.")
                    .into(),
                t.text_tertiary,
            )),
            status_pill,
            cx,
        ))
        .child(endpoint);
    let shown_url = url.unwrap_or_else(|| "http://127.0.0.1:47731/mcp".into());
    let cmd_row = |who: &'static str, hint: Option<&'static str>, id: &str, command: String| {
        div()
            .px(SPACE_4)
            .py(SPACE_2)
            .border_b(crate::theme::border())
            .border_color(t.line_soft)
            .flex()
            .items_start()
            .gap(SPACE_2)
            .child(
                div()
                    .w(px(116.))
                    .pt(SPACE_1H)
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(theme::ui_text(TEXT_SM))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(t.text_primary)
                            .child(who),
                    )
                    .children(hint.map(|hint| {
                        div()
                            .text_size(theme::ui_text(TEXT_2XS))
                            .text_color(t.text_tertiary)
                            .child(hint)
                    })),
            )
            .child(code_block(command.clone()))
            .child(copy(this, id, t!("Copy command"), command))
    };
    let add = div()
        .flex()
        .flex_col()
        .child(cmd_row(
            "Claude Code",
            Some(t!("Then run /mcp")),
            "claude",
            format!("claude mcp add --transport http sikemux {shown_url}"),
        ))
        .child(cmd_row(
            "Codex",
            None,
            "codex",
            format!("codex mcp add sikemux --url {shown_url}"),
        ))
        .child(cmd_row(
            "Codex sign-in",
            None,
            "codex-login",
            "codex mcp login sikemux".into(),
        ))
        .child(rows::note(
            t!("Any other app: add a remote MCP server with OAuth and give it the endpoint above."),
            cx,
        ));
    let ink = t.text_secondary;
    let clients = outside
        .map(|outside| outside.clients.as_slice())
        .unwrap_or_default();
    let allowed_count =
        (!clients.is_empty()).then(|| SharedString::from(clients.len().to_string()));
    let allowed = match clients.is_empty() {
        true => rows::note(
            t!("No app has signed in yet. When one does, Sikemux asks you whether to let it in."),
            cx,
        ),
        false => div()
            .children(clients.iter().map(|client| {
                rows::row_keeping_control(
                    Some(ui::picture(Icon::Terminal, 16., ink)),
                    client.name.clone(),
                    Some(about(client)),
                    remove(client, this),
                    cx,
                )
            }))
            .into_any_element(),
    };
    let opening_mods = this.clone();
    let mods_note = div()
        .flex()
        .items_center()
        .gap(SPACE_2)
        .pt(SPACE_1)
        .text_size(theme::ui_text(TEXT_XS))
        .text_color(t.text_tertiary)
        .child(ui::icon(Icon::InfoCircle, 13.).text_color(t.text_tertiary))
        .child(div().flex_1().min_w_0().child(t!(
            "Mods live in the rail now. Open one there to manage what it can read."
        )))
        .child(
            ui::Button::new("developer-open-mods", t!("Open Mods"), ui::Variant::Ghost).on_click(
                move |_, window, cx| {
                    opening_mods.update(cx, |bench, cx| {
                        crate::settings::close(bench, window, cx);
                        bench.open_rail_on(TabRailPage::Mods, cx);
                    })
                },
            ),
        )
        .into_any_element();
    vec![
        rows::section(n!("MCP server"), None, None, lit("MCP server"), server, cx),
        rows::section(
            n!("Add to an app"),
            None,
            None,
            lit("Add to an app"),
            add,
            cx,
        ),
        rows::section(
            n!("Allowed apps"),
            allowed_count,
            Some(t!("A sign-in lasts 30 days. Removing an app cuts it off at once.").into()),
            lit("Allowed apps"),
            allowed,
            cx,
        ),
        mods_note,
    ]
}
