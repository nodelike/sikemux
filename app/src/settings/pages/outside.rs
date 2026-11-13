//! Apps outside Sikemux that use its chats through MCP: the address to give them, how to add
//! it to Claude Code and Codex, and the apps let in, to change or remove.

use gpui_kit::{
    div, AnyElement, App, ClipboardItem, Entity, IntoElement, ParentElement, SharedString, Styled,
};
use sikemux_core::server::call;
use sikemux_core::server::outside::{OutsideChange, OutsideClient, Via};
use sikemux_wire::protocol::DeviceAccess;

use crate::icons::Icon;
use crate::settings::index::Page::OtherApps;
use crate::settings::index::{section, Entry};
use crate::settings::rows;
use crate::theme::{self, Tokens, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_4, TEXT_XS};
use crate::ui;
use crate::workbench::Workbench;

/// What a search finds on this page.
pub(in crate::settings) const ENTRIES: &[Entry] = &[
    section(
        OtherApps,
        "MCP server",
        "mcp server url address claude code codex chatgpt outside apps agents oauth",
    ),
    section(
        OtherApps,
        "Add to an app",
        "claude mcp add codex mcp add login connect",
    ),
    section(
        OtherApps,
        "Allowed apps",
        "apps signed in revoke remove access full watch read only",
    ),
];

fn copy(this: &Entity<Workbench>, id: &str, text: String) -> ui::Button {
    let copying = this.clone();
    ui::Button::new(
        SharedString::from(format!("outside-copy-{id}")),
        t!("Copy"),
        ui::Variant::Outline,
    )
    .icon(Icon::Copy)
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
    let (removing, id) = (this.clone(), client.id.clone());
    ui::Button::new(
        SharedString::from(format!("outside-remove-{}", client.id)),
        t!("Remove"),
        ui::Variant::Outline,
    )
    .icon(Icon::Trash)
    .on_click(move |_, _, cx| {
        let id = id.clone();
        removing.update(cx, |bench, cx| {
            remove_app(bench, id);
            bench.notify(
                crate::toast::success(t!("Removed. It can no longer use your chats.")),
                cx,
            );
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
    let code = |text: String| {
        div()
            .flex_1()
            .min_w_0()
            .px(SPACE_1H)
            .py(SPACE_1)
            .rounded(theme::RADIUS_2)
            .bg(t.surface_sunken)
            .font_family(theme::MONO)
            .text_size(crate::theme::ui_text(TEXT_XS))
            .text_color(t.text_primary)
            .whitespace_normal()
            .child(text)
    };
    let line = |id: &str, text: String| {
        div()
            .flex()
            .items_center()
            .gap(SPACE_2)
            .child(code(text.clone()))
            .child(copy(this, id, text))
    };
    let server = match (&url, outside.and_then(|outside| outside.error.clone())) {
        (Some(url), _) => div()
            .p(SPACE_4)
            .child(line("url", url.clone()))
            .into_any_element(),
        (None, Some(error)) => {
            rows::note(t!("The server is not running: {error}", error = error), cx)
        }
        (None, None) => rows::note(t!("Starting…"), cx),
    };
    let label = |text: &str| {
        div()
            .text_size(crate::theme::ui_text(TEXT_XS))
            .text_color(t.text_tertiary)
            .child(text.to_owned())
    };
    let shown_url = url.unwrap_or_else(|| "http://127.0.0.1:47731/mcp".into());
    let add = div()
        .flex()
        .flex_col()
        .gap(SPACE_2H)
        .p(SPACE_4)
        .child(label(t!("Claude Code, then run /mcp in it to sign in")))
        .child(line(
            "claude",
            format!("claude mcp add --transport http sikemux {shown_url}"),
        ))
        .child(label("Codex"))
        .child(line(
            "codex",
            format!("codex mcp add sikemux --url {shown_url}"),
        ))
        .child(line("codex-login", "codex mcp login sikemux".into()))
        .child(label(t!(
            "Any other app: add a remote MCP server with OAuth and give it the address."
        )));
    let ink = t.text_secondary;
    let clients = outside
        .map(|outside| outside.clients.as_slice())
        .unwrap_or_default();
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
    vec![
        rows::section(
            n!("MCP server"),
            None,
            Some(t!("Apps on this computer, such as Claude Code and Codex in your own terminal, list, start, message and read your chats here. Only apps on this computer can sign in.").into()),
            lit("MCP server"),
            server,
            cx,
        ),
        rows::section(n!("Add to an app"), None, None, lit("Add to an app"), add, cx),
        rows::section(
            n!("Allowed apps"),
            None,
            Some(t!("A sign-in lasts 30 days. Removing an app cuts it off at once.").into()),
            lit("Allowed apps"),
            allowed,
            cx,
        ),
    ]
}
