//! Runtime diagnostics: a redacted snapshot of this window and the core's terminals, the
//! agent detection manifests the core runs on, and why the core reads an agent's terminal
//! as it does. Nothing it shows is a terminal's text, an environment value or a credential.

use std::collections::BTreeMap;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, relative, AnyElement, App, ClipboardItem, Context, Entity, FocusHandle, FontWeight,
    Hsla, InteractiveElement, IntoElement, MouseButton, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window,
};
use serde_json::{json, Value};
use tokio::sync::oneshot;

use crate::core::{Ask, Detection};
use crate::experience::{self, Experience};
use crate::theme::{self, Tokens};
use crate::ui;
use crate::workbench::Workbench;

#[derive(Default)]
pub struct Diagnostics {
    manifests: Option<Value>,
    explain: Option<Value>,
    error: Option<String>,
    /// Where the snapshot was last saved, once it was.
    saved: Option<String>,
    /// Holds the keys while it is up, so the shell's shortcuts stand aside.
    focus: Option<FocusHandle>,
}

/// Takes the keys the first time it is drawn.
pub fn take_keys(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    if let Some(diagnostics) = diagnostics_of(bench).filter(|shown| shown.focus.is_none()) {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        diagnostics.focus = Some(focus);
    }
}

fn diagnostics_of(bench: &mut Workbench) -> Option<&mut Diagnostics> {
    match bench.experience.as_mut()? {
        Experience::Diagnostics(diagnostics) => Some(diagnostics),
        _ => None,
    }
}

/// Asks the core for its manifests, or to read them again.
pub fn opened(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    ask(bench, Detection::List, cx);
}

fn ask(bench: &Workbench, detection: Detection, cx: &mut Context<Workbench>) {
    let explaining = matches!(detection, Detection::Explain(_));
    let (reply, answer) = oneshot::channel();
    let _ = bench.asks.send(Ask::Detection { detection, reply });
    cx.spawn(async move |bench, cx| {
        let answer = answer
            .await
            .unwrap_or_else(|_| Err("the core is not connected".into()));
        let _ = bench.update(cx, |bench, cx| {
            if let Some(diagnostics) = diagnostics_of(bench) {
                match (answer, explaining) {
                    (Ok(explain), true) => diagnostics.explain = Some(explain),
                    (Ok(manifests), false) => diagnostics.manifests = Some(manifests),
                    (Err(error), _) => diagnostics.error = Some(error),
                }
            }
            bench.parts.redraw(&bench.parts.over, cx);
        });
    })
    .detach();
}

/// This window's process and the core's terminals, counted, never read.
fn snapshot(bench: &Workbench) -> Value {
    let (limit_soft, limit_hard) = fd_limit();
    let sessions = bench
        .view
        .as_ref()
        .map_or(&[][..], |view| view.sessions.as_slice());
    let mut agents: BTreeMap<String, u32> = BTreeMap::new();
    for session in sessions.iter().filter(|session| session.agent_id.is_some()) {
        let state = session
            .status
            .and_then(|status| serde_json::to_value(status).ok())
            .and_then(|status| status.as_str().map(str::to_owned))
            .unwrap_or_else(|| "unknown".into());
        *agents.entry(state).or_default() += 1;
    }
    json!({
        "window": {
            "pid": std::process::id(),
            "fdCount": fd_count(),
            "fdLimitSoft": limit_soft,
            "fdLimitHard": limit_hard,
        },
        "terminals": {
            "sessions": sessions.len(),
            "shown": bench.terminals.open_count(),
            "agentTerminals": agents,
        },
    })
}

/// How many files this process has open, counted from `/dev/fd`.
#[cfg(unix)]
fn fd_count() -> Option<usize> {
    std::fs::read_dir("/dev/fd").ok().map(Iterator::count)
}

#[cfg(windows)]
fn fd_count() -> Option<usize> {
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessHandleCount};

    let mut count = 0;
    // SAFETY: the pseudo handle needs no closing, and `count` is a live local.
    (unsafe { GetProcessHandleCount(GetCurrentProcess(), &mut count) } != 0)
        .then_some(count as usize)
}

#[cfg(unix)]
fn fd_limit() -> (Option<u64>, Option<u64>) {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: getrlimit writes one rlimit into the struct it is given.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
        return (None, None);
    }
    (Some(limit.rlim_cur), Some(limit.rlim_max))
}

#[cfg(windows)]
fn fd_limit() -> (Option<u64>, Option<u64>) {
    (None, None)
}

fn snapshot_text(bench: &Workbench) -> String {
    serde_json::to_string_pretty(&snapshot(bench)).unwrap_or_default()
}

pub fn render(
    bench: &Workbench,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let Some(Experience::Diagnostics(diagnostics)) = &bench.experience else {
        return div().into_any_element();
    };
    let t = Tokens::get(cx).clone();
    let close = {
        let this = this.clone();
        move |_: &gpui_kit::MouseDownEvent, _: &mut Window, cx: &mut App| {
            this.update(cx, experience::close)
        }
    };
    let kicker = |text: &'static str| {
        ui::text(text.to_uppercase(), 9., FontWeight::SEMIBOLD)
            .font_family(theme::MONO)
            .letter_spacing(0.08)
            .text_color(t.accent)
    };
    let mono_block = |text: String, ink: Hsla| {
        div()
            .px(px(30.))
            .py(px(20.))
            .bg(t.surface_sunken)
            .font_family(theme::MONO)
            .text_size(crate::theme::ui_text(10.5))
            .line_height(relative(1.55))
            .text_color(ink)
            .child(text)
    };
    let manifests = diagnostics
        .manifests
        .as_ref()
        .and_then(|report| report["manifests"].as_array())
        .map_or(&[][..], Vec::as_slice);
    let signal = |title: String, detail: String| {
        div()
            .flex()
            .flex_col()
            .gap(px(3.))
            .p(px(8.))
            .bg(t.surface)
            .text_size(crate::theme::ui_text(10.))
            .text_color(t.text_secondary)
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.text_primary)
                    .child(title),
            )
            .child(detail)
    };
    let agents = bench.view.as_ref().map_or(Vec::new(), |view| {
        let mut projects: Vec<&str> = view
            .workspace
            .agents
            .iter()
            .map(|agent| agent.project.as_str())
            .collect();
        projects.dedup();
        projects
            .into_iter()
            .flat_map(|project| bench.agents_of(project))
            .collect()
    });
    let signals = div()
        .px(px(30.))
        .py(px(14.))
        .flex()
        .flex_wrap()
        .gap(px(1.))
        .bg(t.border)
        .child(
            div()
                .w_full()
                .pb(px(8.))
                .pt(px(3.))
                .bg(t.surface)
                .child(kicker("agent detection manifests")),
        )
        .children(manifests.iter().map(|manifest| {
            let source = manifest["source"]["kind"].as_str().unwrap_or_default();
            let warning = if manifest["warning"].is_null() {
                ""
            } else {
                " · warning"
            };
            signal(
                manifest["agent"].as_str().unwrap_or_default().to_owned(),
                format!(
                    "v{} · {source}{warning}",
                    manifest["version"].as_str().unwrap_or_default()
                ),
            )
            .w(relative(0.195))
        }))
        .children(agents.iter().map(|agent| {
            let (this, id) = (this.clone(), agent.id.clone());
            let status = serde_json::to_value(agent.status)
                .ok()
                .and_then(|status| status.as_str().map(str::to_owned))
                .unwrap_or_default();
            div()
                .id(SharedString::from(format!(
                    "diagnostics-explain-{}",
                    agent.id
                )))
                .w(relative(0.195))
                .cursor_pointer()
                .on_click(move |_, _, cx| {
                    let id = id.clone();
                    this.update(cx, |bench, cx| ask(bench, Detection::Explain(id), cx))
                })
                .child(signal(agent.title.clone(), format!("{status} · explain")))
        }));
    let button = |id: &'static str,
                  label: &'static str,
                  run: fn(&mut Workbench, &mut Context<Workbench>)| {
        let this = this.clone();
        ui::Button::new(id, label, ui::Variant::Outline)
            .on_click(move |_, _, cx| this.update(cx, run))
    };
    let card = div()
        .id("diagnostics")
        .relative()
        .w(px(720.))
        .max_w_full()
        .max_h(relative(0.9))
        .flex()
        .flex_col()
        .overflow_hidden()
        .bg(t.surface)
        .border_1()
        .border_color(t.border)
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(div().absolute().top_0().bottom_0().left_0().w(px(4.)).bg(t.accent))
        .child(
            div()
                .flex()
                .justify_between()
                .pl(px(30.))
                .pr(px(26.))
                .pt(px(22.))
                .pb(px(16.))
                .border_b_1()
                .border_color(t.border)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .child(kicker("Sikemux signal deck"))
                        .child(
                            ui::text("Runtime diagnostics", 22., FontWeight::MEDIUM)
                                .mt(px(3.))
                                .letter_spacing(-0.03)
                                .text_color(t.text_primary),
                        ),
                )
                .child({
                    let this = this.clone();
                    div()
                        .id("diagnostics-esc")
                        .font_family(theme::MONO)
                        .text_size(crate::theme::ui_text(10.))
                        .text_color(t.text_tertiary)
                        .cursor_pointer()
                        .on_click(move |_, _, cx| this.update(cx, experience::close))
                        .child("esc")
                }),
        )
        .child(
            div()
                .px(px(30.))
                .py(px(18.))
                .border_b_1()
                .border_color(t.border)
                .text_size(crate::theme::ui_text(12.))
                .line_height(relative(1.6))
                .text_color(t.text_secondary)
                .child("A redacted operational snapshot. Terminal text, environment values, credentials, and API secrets are never included."),
        )
        .child(
            div()
                .id("diagnostics-body")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(signals)
                .children(diagnostics.explain.as_ref().map(|explain| {
                    mono_block(serde_json::to_string_pretty(explain).unwrap_or_default(), t.warn)
                        .border_b_1()
                        .border_color(t.border)
                }))
                .child(match &diagnostics.error {
                    Some(error) => div().px(px(30.)).py(px(20.)).text_color(t.danger).child(error.clone()),
                    None => mono_block(snapshot_text(bench), t.live).min_h(px(280.)),
                }),
        )
        .child(
            div()
                .flex()
                .justify_end()
                .items_center()
                .gap(px(8.))
                .px(px(26.))
                .py(px(14.))
                .border_t_1()
                .border_color(t.border)
                .children(diagnostics.saved.clone().map(|path| {
                    div().flex_1().truncate().text_size(crate::theme::ui_text(11.)).text_color(t.text_tertiary).child(format!("Saved to {path}"))
                }))
                .child(button("diagnostics-reload", "Reload manifests", |bench, cx| ask(bench, Detection::Reload, cx)))
                .child(button("diagnostics-refresh", "Refresh", |bench, cx| {
                    if let Some(diagnostics) = diagnostics_of(bench) {
                        diagnostics.error = None;
                    }
                    ask(bench, Detection::List, cx);
                }))
                .child(button("diagnostics-copy", "Copy JSON", |bench, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(snapshot_text(bench)));
                }))
                .child(button("diagnostics-save", "Save JSON", |bench, cx| save(bench, cx))),
        );
    div()
        .id("diagnostics-backdrop")
        .key_context("Diagnostics Modal")
        .when_some(diagnostics.focus.as_ref(), |backdrop, focus| {
            backdrop.track_focus(focus)
        })
        .absolute()
        .inset_0()
        .p(px(24.))
        .flex()
        .items_center()
        .justify_center()
        .bg(Hsla {
            a: 0.76,
            ..t.surface_sunken
        })
        .occlude()
        .on_mouse_down(MouseButton::Left, close)
        .on_key_down({
            let this = this.clone();
            move |event, _, cx| {
                if event.keystroke.key == "escape" {
                    this.update(cx, experience::close);
                }
            }
        })
        .child(card)
        .into_any_element()
}

/// Asks where to keep the snapshot, then writes it there.
fn save(bench: &Workbench, cx: &mut Context<Workbench>) {
    let text = snapshot_text(bench);
    let stamp = chrono::Utc::now().format("%Y-%m-%dT%H-%M-%S");
    let home = sikemux_process::paths::home().unwrap_or_default();
    let picked = cx.prompt_for_new_path(&home, Some(&format!("sikemux-diagnostics-{stamp}.json")));
    cx.spawn(async move |bench, cx| {
        let Ok(Ok(Some(path))) = picked.await else {
            return;
        };
        let written = std::fs::write(&path, format!("{text}\n"));
        let _ = bench.update(cx, |bench, cx| {
            if let Some(diagnostics) = diagnostics_of(bench) {
                match written {
                    Ok(()) => diagnostics.saved = Some(path.display().to_string()),
                    Err(error) => diagnostics.error = Some(error.to_string()),
                }
            }
            bench.parts.redraw(&bench.parts.over, cx);
        });
    })
    .detach();
}
