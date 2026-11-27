//! Native in-chat Agentic Vault cards: first-entry forms (`vault_request`) and gated blind-fill
//! approvals (`VaultPendingApproval`), rendered directly by the host so agents never see values.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, AppContext, Context, Entity, FontWeight, InteractiveElement,
    IntoElement, ParentElement, PathPromptOptions, SharedString, Styled, Subscription, WeakEntity,
    Window,
};
use sikemux_calls::call;
use sikemux_calls::vault::{
    VaultApprovalAnswerParams, VaultCategory, VaultDocumentInput, VaultFieldInput, VaultFieldKind,
    VaultPendingApproval, VaultPendingRequest, VaultRequestCancelParams, VaultRequestSubmitParams,
    VaultSource,
};

use crate::icons::Icon;
use crate::theme::{
    self, Tokens, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_3, SPACE_4, TEXT_2XS, TEXT_3XS,
    TEXT_SM, TEXT_XS,
};
use crate::toast;
use crate::ui::{self, Clicks};
use crate::vault_ui::{self, CardBrand};
use crate::workbench::Workbench;

pub struct VaultCards {
    requests: HashMap<String, RequestForm>,
    approvals: HashMap<String, ApprovalForm>,
    signal: Entity<()>,
}

impl VaultCards {
    pub fn new(cx: &mut App) -> Self {
        Self {
            requests: HashMap::new(),
            approvals: HashMap::new(),
            signal: cx.new(|_| ()),
        }
    }

    pub(crate) fn signal(&self) -> &Entity<()> {
        &self.signal
    }

    pub(crate) fn notify(&self, cx: &mut App) {
        self.signal.update(cx, |_, cx| cx.notify());
    }
}

struct CardField {
    key: String,
    label: String,
    kind: VaultFieldKind,
    required: bool,
    sensitive: bool,
    masked: bool,
    input: Entity<InputState>,
}

struct RequestForm {
    source_onepassword: bool,
    approve_domain: bool,
    op_ref: Entity<InputState>,
    fields: Vec<CardField>,
    doc_path: Option<PathBuf>,
    busy: bool,
    error: Option<SharedString>,
    _subs: Vec<Subscription>,
}

struct ApprovalForm {
    remember_domain: bool,
    cvc: Option<Entity<InputState>>,
    busy: bool,
    error: Option<SharedString>,
    _subs: Vec<Subscription>,
}

/// Ensures every open Vault first-entry request and approval has its native GPUI `InputState`s,
/// and drops forms once resolved. Zero work when nothing is pending.
pub fn sync(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    let requests: Vec<VaultPendingRequest> = bench.feeds.vault_requests().cloned().collect();
    let approvals: Vec<VaultPendingApproval> = bench.feeds.vault_approvals().cloned().collect();

    if requests.is_empty()
        && approvals.is_empty()
        && bench.vault_cards.requests.is_empty()
        && bench.vault_cards.approvals.is_empty()
    {
        return;
    }

    let mut changed = false;
    let live_reqs: HashSet<&str> = requests.iter().map(|r| r.id.as_str()).collect();
    let reqs_before = bench.vault_cards.requests.len();
    bench
        .vault_cards
        .requests
        .retain(|id, _| live_reqs.contains(id.as_str()));
    if bench.vault_cards.requests.len() != reqs_before {
        changed = true;
    }

    let live_apps: HashSet<&str> = approvals.iter().map(|a| a.id.as_str()).collect();
    let apps_before = bench.vault_cards.approvals.len();
    bench
        .vault_cards
        .approvals
        .retain(|id, _| live_apps.contains(id.as_str()));
    if bench.vault_cards.approvals.len() != apps_before {
        changed = true;
    }

    for req in requests {
        if bench.vault_cards.requests.contains_key(&req.id) {
            continue;
        }
        changed = true;
        let mut subs = Vec::new();
        let op_ref =
            cx.new(|cx| InputState::new(window, cx).placeholder("op://Personal/Item/credential"));
        let mut fields = Vec::with_capacity(req.fields.len());
        let is_payment = req.category == VaultCategory::Payments;
        for f in &req.fields {
            if f.kind == VaultFieldKind::Document {
                continue;
            }
            let prefill = f.value.as_deref().map(str::trim).filter(|v| !v.is_empty());
            let masked = (f.sensitive || f.kind.is_inherently_sensitive()) && prefill.is_none();
            let ph = f.placeholder.clone().unwrap_or_else(|| match f.kind {
                VaultFieldKind::CardNumber => "4242 4242 4242 4242".to_owned(),
                VaultFieldKind::Expiry => "MM/YY".to_owned(),
                VaultFieldKind::Cvc => "CVC".to_owned(),
                VaultFieldKind::Totp => "Base32 2FA seed".to_owned(),
                _ => f.label.clone(),
            });
            let input = cx.new(|cx| {
                let mut state = InputState::new(window, cx).placeholder(ph);
                if let Some(val) = prefill {
                    state = state.default_value(val.to_owned());
                }
                if masked {
                    state = state.masked(true);
                }
                state
            });
            if is_payment {
                subs.push(cx.subscribe(&input, |bench, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        bench.vault_cards.notify(cx);
                    }
                }));
            }
            fields.push(CardField {
                key: f.key.clone(),
                label: f.label.clone(),
                kind: f.kind,
                required: f.required,
                sensitive: f.sensitive,
                masked,
                input,
            });
        }
        let doc_path = req
            .proposed_document
            .as_ref()
            .map(|doc| PathBuf::from(&doc.source_path));
        bench.vault_cards.requests.insert(
            req.id,
            RequestForm {
                source_onepassword: false,
                approve_domain: req.category != VaultCategory::Payments,
                op_ref,
                fields,
                doc_path,
                busy: false,
                error: None,
                _subs: subs,
            },
        );
    }

    for app in approvals {
        if bench.vault_cards.approvals.contains_key(&app.id) {
            continue;
        }
        changed = true;
        let subs = Vec::new();
        let needs_cvc = app.category == VaultCategory::Payments
            && (app.field_keys.iter().any(|k| k == "cvc" || k == "cvv")
                || app.field_keys.is_empty());
        let cvc = needs_cvc.then(|| {
            cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(t!("CVC (optional · never saved)"))
                    .masked(true)
            })
        });
        bench.vault_cards.approvals.insert(
            app.id,
            ApprovalForm {
                remember_domain: false,
                cvc,
                busy: false,
                error: None,
                _subs: subs,
            },
        );
    }

    if changed {
        bench.vault_cards.notify(cx);
    }
}

/// Renders all open Vault first-entry and approval cards for `agent`.
pub(super) fn cards_for(
    bench: &Workbench,
    agent: &str,
    handle: WeakEntity<Workbench>,
    cx: &App,
) -> Vec<AnyElement> {
    let mut out = Vec::new();
    for req in bench.feeds.vault_requests().filter(|r| r.agent_id == agent) {
        if let Some(form) = bench.vault_cards.requests.get(&req.id) {
            out.push(render_request_card(req, form, handle.clone(), cx));
        }
    }
    for app in bench
        .feeds
        .vault_approvals()
        .filter(|a| a.agent_id == agent)
    {
        if let Some(form) = bench.vault_cards.approvals.get(&app.id) {
            out.push(render_approval_card(app, form, handle.clone(), cx));
        }
    }
    out
}

fn pick_request_doc(
    _bench: &mut Workbench,
    request_id: String,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    let picked = cx.prompt_for_paths(PathPromptOptions {
        files: true,
        directories: false,
        multiple: false,
        prompt: Some(t!("Attach document for blind upload").into()),
    });
    cx.spawn_in(window, async move |bench, cx| {
        if let Ok(Ok(Some(paths))) = picked.await {
            if let Some(path) = paths.into_iter().next() {
                let _ = bench.update(cx, |bench, cx| {
                    if let Some(form) = bench.vault_cards.requests.get_mut(&request_id) {
                        form.doc_path = Some(path);
                        form.error = None;
                    }
                    bench.vault_cards.notify(cx);
                });
            }
        }
    })
    .detach();
}

fn submit_request_card(
    bench: &mut Workbench,
    request_id: String,
    ephemeral: bool,
    cx: &mut Context<Workbench>,
) {
    let Some(req) = bench
        .feeds
        .vault_requests()
        .find(|r| r.id == request_id)
        .cloned()
    else {
        return;
    };
    let built: Result<VaultRequestSubmitParams, String> = (|| {
        let form = bench
            .vault_cards
            .requests
            .get(&request_id)
            .ok_or_else(|| "request form not found".to_owned())?;

        let source = if req.category == VaultCategory::Secrets && form.source_onepassword {
            let reference = form.op_ref.read(cx).value().trim().to_owned();
            if !reference.starts_with("op://") {
                return Err(t!("1Password reference must start with op://").to_owned());
            }
            VaultSource::OnePassword { reference }
        } else {
            VaultSource::Sikemux
        };

        let document = if let Some(path) = &form.doc_path {
            let filename = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "document.bin".to_owned());
            Some(VaultDocumentInput {
                filename,
                mime_type: None,
                source_path: Some(path.to_string_lossy().into_owned()),
                data_base64: None,
            })
        } else {
            None
        };

        if req.category == VaultCategory::Documents && document.is_none() {
            return Err(t!("Choose a file to attach first.").to_owned());
        }

        let mut fields = Vec::new();
        if let VaultSource::OnePassword { ref reference } = source {
            let key = req
                .fields
                .first()
                .map(|f| f.key.clone())
                .unwrap_or_else(|| "secret".to_owned());
            fields.push(VaultFieldInput {
                key,
                label: "1Password reference".to_owned(),
                kind: VaultFieldKind::Secret,
                sensitive: true,
                value: None,
                op_ref: Some(reference.clone()),
            });
        } else {
            for f in &form.fields {
                let val = f.input.read(cx).value().trim().to_owned();
                if val.is_empty() {
                    if f.required {
                        return Err(t!("Enter {field} to continue.", field = f.label.clone()));
                    }
                    continue;
                }
                fields.push(VaultFieldInput {
                    key: f.key.clone(),
                    label: f.label.clone(),
                    kind: f.kind,
                    sensitive: f.sensitive,
                    value: Some(val),
                    op_ref: None,
                });
            }
        }

        Ok(VaultRequestSubmitParams {
            request_id: request_id.clone(),
            existing_entry_id: None,
            title: Some(req.title.clone()),
            scope: Some(req.scope.clone()),
            source,
            domains: req.domain.clone().into_iter().collect(),
            require_approval: matches!(
                req.category,
                VaultCategory::Secrets | VaultCategory::Payments | VaultCategory::Documents
            ),
            fields,
            document,
            approve_domain: form.approve_domain,
            ephemeral,
        })
    })();

    let params = match built {
        Ok(params) => params,
        Err(message) => {
            if let Some(form) = bench.vault_cards.requests.get_mut(&request_id) {
                form.error = Some(message.into());
            }
            bench.vault_cards.notify(cx);
            return;
        }
    };

    if let Some(form) = bench.vault_cards.requests.get_mut(&request_id) {
        form.busy = true;
        form.error = None;
    }
    bench.vault_cards.notify(cx);

    let target = bench.chat_target(&req.agent_id);
    let asked = target.request_typed::<call::VaultRequestSubmit>(params);
    let title = req.title.clone();
    let who = vault_ui::format_actor(&req.agent_name);
    cx.spawn(async move |bench, cx| {
        let answer = asked.await;
        let _ = bench.update(cx, |bench, cx| match answer {
            Ok(_) => {
                bench.vault_cards.requests.remove(&request_id);
                let msg = if ephemeral {
                    t!(
                        "Shared “{title}” once for {who} · not saved to Vault",
                        title = title,
                        who = who
                    )
                } else {
                    t!(
                        "Saved “{title}” to Vault · {who} can blind-fill",
                        title = title,
                        who = who
                    )
                };
                bench.notify(toast::success(msg), cx);
                bench.vault_cards.notify(cx);
            }
            Err(error) => {
                if let Some(form) = bench.vault_cards.requests.get_mut(&request_id) {
                    form.busy = false;
                    form.error = Some(error.clone().into());
                }
                bench.notify(toast::error(error), cx);
                bench.vault_cards.notify(cx);
            }
        });
    })
    .detach();
}

fn cancel_request_card(bench: &mut Workbench, request_id: String, cx: &mut Context<Workbench>) {
    let Some(req) = bench
        .feeds
        .vault_requests()
        .find(|r| r.id == request_id)
        .cloned()
    else {
        return;
    };
    if let Some(form) = bench.vault_cards.requests.get_mut(&request_id) {
        form.busy = true;
    }
    bench.vault_cards.notify(cx);
    let target = bench.chat_target(&req.agent_id);
    let asked = target.request_typed::<call::VaultRequestCancel>(VaultRequestCancelParams {
        request_id: request_id.clone(),
        reason: Some("Declined by person".to_owned()),
    });
    let who = vault_ui::format_actor(&req.agent_name);
    cx.spawn(async move |bench, cx| {
        let answer = asked.await;
        let _ = bench.update(cx, |bench, cx| match answer {
            Ok(()) => {
                bench.vault_cards.requests.remove(&request_id);
                bench.notify(
                    toast::info(t!("Declined Vault request · {who} was told", who = who)),
                    cx,
                );
                bench.vault_cards.notify(cx);
            }
            Err(error) => {
                if let Some(form) = bench.vault_cards.requests.get_mut(&request_id) {
                    form.busy = false;
                    form.error = Some(error.clone().into());
                }
                bench.notify(toast::error(error), cx);
                bench.vault_cards.notify(cx);
            }
        });
    })
    .detach();
}

fn answer_approval_card(
    bench: &mut Workbench,
    approval_id: String,
    approved: bool,
    cx: &mut Context<Workbench>,
) {
    let Some(app) = bench
        .feeds
        .vault_approvals()
        .find(|a| a.id == approval_id)
        .cloned()
    else {
        return;
    };
    let (remember_domain, ephemeral_values) = {
        let Some(form) = bench.vault_cards.approvals.get(&approval_id) else {
            return;
        };
        let mut ephemeral = BTreeMap::new();
        if approved {
            if let Some(cvc_input) = &form.cvc {
                let cvc = cvc_input.read(cx).value().trim().to_owned();
                if !cvc.is_empty() {
                    ephemeral.insert("cvc".to_owned(), cvc);
                }
            }
        }
        (form.remember_domain, ephemeral)
    };
    if let Some(form) = bench.vault_cards.approvals.get_mut(&approval_id) {
        form.busy = true;
        form.error = None;
    }
    bench.vault_cards.notify(cx);

    let target = bench.chat_target(&app.agent_id);
    let asked = target.request_typed::<call::VaultApprovalAnswer>(VaultApprovalAnswerParams {
        id: approval_id.clone(),
        approved,
        remember_domain,
        ephemeral_values,
    });
    let title = app.entry_title.clone();
    let domain = if app.domain.is_empty() {
        t!("this page").to_owned()
    } else {
        app.domain.clone()
    };
    cx.spawn(async move |bench, cx| {
        let answer = asked.await;
        let _ = bench.update(cx, |bench, cx| match answer {
            Ok(()) => {
                bench.vault_cards.approvals.remove(&approval_id);
                let note = if approved {
                    toast::success(t!(
                        "Approved blind-fill of “{title}” on {domain}",
                        title = title,
                        domain = domain
                    ))
                } else {
                    toast::info(t!(
                        "Denied blind-fill of “{title}” on {domain}",
                        title = title,
                        domain = domain
                    ))
                };
                bench.notify(note, cx);
                bench.vault_cards.notify(cx);
            }
            Err(error) => {
                if let Some(form) = bench.vault_cards.approvals.get_mut(&approval_id) {
                    form.busy = false;
                    form.error = Some(error.clone().into());
                }
                bench.notify(toast::error(error), cx);
                bench.vault_cards.notify(cx);
            }
        });
    })
    .detach();
}

fn card_frame(child: impl IntoElement, cx: &App) -> AnyElement {
    div()
        .w_full()
        .flex()
        .justify_center()
        .child(
            div()
                .w_full()
                .max_w(super::column::rows(cx))
                .px(px(22.))
                .child(child),
        )
        .into_any_element()
}

fn render_request_card(
    req: &VaultPendingRequest,
    form: &RequestForm,
    handle: WeakEntity<Workbench>,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let color = vault_ui::category_color(req.category, &t);
    let who = vault_ui::format_actor(&req.agent_name);
    let req_id = req.id.clone();

    let bar = div()
        .px(SPACE_3)
        .py(SPACE_2)
        .border_b(theme::border())
        .border_color(t.border)
        .bg(t.surface_sunken.opacity(0.6))
        .flex()
        .items_center()
        .gap(SPACE_2)
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_1H)
                .text_size(theme::ui_text(TEXT_2XS))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.text_primary)
                .child(ui::icon(Icon::ShieldCheck, 12.).text_color(color))
                .child(t!("Sikemux Vault")),
        )
        .child(
            div()
                .text_size(theme::ui_text(TEXT_2XS))
                .text_color(t.text_tertiary)
                .child(t!("Native host form")),
        )
        .child(vault_ui::pill(
            vault_ui::category_label(req.category),
            color,
            None,
            cx,
        ))
        .child(div().flex_1())
        .children(req.domain.as_deref().map(|d| vault_ui::domain_chip(d, cx)));

    let source_toggle = (req.category == VaultCategory::Secrets).then(|| {
        let id = req_id.clone();
        let selected = usize::from(form.source_onepassword);
        let source_now = if form.source_onepassword {
            VaultSource::OnePassword {
                reference: String::new(),
            }
        } else {
            VaultSource::Sikemux
        };
        let bench = handle.clone();
        div()
            .flex()
            .flex_col()
            .gap(SPACE_2)
            .child(
                ui::Segmented::words(
                    SharedString::from(format!("vreq-src-{id}")),
                    vec![
                        (SharedString::from(t!("Sikemux Vault")), None),
                        (SharedString::from("1Password (op://)"), None),
                    ],
                    selected,
                )
                .glyph(0, Icon::Lock)
                .glyph(1, Icon::OnePassword)
                .on_select(move |ix, _, cx| {
                    let is_op = *ix == 1;
                    let _ = bench.update(cx, |bench, cx| {
                        if let Some(form) = bench.vault_cards.requests.get_mut(&id) {
                            form.source_onepassword = is_op;
                            form.error = None;
                        }
                        bench.vault_cards.notify(cx);
                    });
                }),
            )
            .child(vault_ui::honest_privacy_badge(&source_now, cx))
    });

    let payment_preview = (req.category == VaultCategory::Payments).then(|| {
        let num = form
            .fields
            .iter()
            .find(|f| f.kind == VaultFieldKind::CardNumber)
            .map(|f| f.input.read(cx).value().to_string())
            .unwrap_or_default();
        let holder = form
            .fields
            .iter()
            .find(|f| f.key == "cardholder")
            .map(|f| f.input.read(cx).value().to_string())
            .unwrap_or_default();
        let exp = form
            .fields
            .iter()
            .find(|f| f.kind == VaultFieldKind::Expiry)
            .map(|f| f.input.read(cx).value().to_string())
            .unwrap_or_default();
        vault_ui::dark_glass_card(
            CardBrand::detect(&num, &req.title),
            &vault_ui::mask_card_last4(&num),
            &holder,
            &exp,
            Some(&req.title),
            cx,
        )
    });

    let doc_dropzone = (req.category == VaultCategory::Documents).then(|| {
        let id = req_id.clone();
        let bench_pick = handle.clone();
        let (name, sub, ready) = if let Some(path) = &form.doc_path {
            if let Some(proposed) = req
                .proposed_document
                .as_ref()
                .filter(|doc| path.to_string_lossy() == doc.source_path)
            {
                let sz = vault_ui::format_bytes(proposed.byte_len);
                (
                    proposed.filename.clone(),
                    format!(
                        "{} · {sz} · {}",
                        proposed.display_path,
                        t!("Click to change")
                    ),
                    true,
                )
            } else {
                let fname = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.to_string_lossy().into_owned());
                let sz = std::fs::metadata(path)
                    .map(|m| vault_ui::format_bytes(m.len()))
                    .unwrap_or_else(|_| t!("Ready").to_owned());
                (
                    fname,
                    format!(
                        "{sz} · {} · {}",
                        t!("encrypted locally"),
                        t!("Click to change")
                    ),
                    true,
                )
            }
        } else {
            (
                t!("Choose a file to encrypt & upload blind").to_owned(),
                t!("PDF, image, or ID · agents never read the file").to_owned(),
                false,
            )
        };
        div()
            .id(SharedString::from(format!("vreq-doc-{id}")))
            .w_full()
            .px(SPACE_3)
            .py(SPACE_2)
            .rounded(theme::radius_2())
            .border(theme::border())
            .when(!ready, |b| b.border_dashed().border_color(t.border_strong))
            .when(ready, |b| b.border_color(t.live.opacity(0.45)))
            .bg(t.surface)
            .hover(move |s| s.bg(t.surface_hover))
            .cursor_pointer()
            .flex()
            .items_center()
            .gap(SPACE_3)
            .clicks(move |_, window, cx| {
                let _ = bench_pick.update(cx, |bench, cx| {
                    pick_request_doc(bench, id.clone(), window, cx)
                });
            })
            .child(ui::icon(Icon::File, 16.).text_color(if ready { t.live } else { color }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(SPACE_0)
                    .child(
                        div()
                            .truncate()
                            .text_size(theme::ui_text(TEXT_XS))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(t.text_primary)
                            .child(name),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(theme::ui_text(TEXT_3XS))
                            .text_color(t.text_tertiary)
                            .child(sub),
                    ),
            )
            .when(ready, |b| {
                b.child(vault_ui::pill(t!("Ready"), t.live, Some(Icon::Check), cx))
            })
    });

    let inputs: Vec<AnyElement> =
        if req.category == VaultCategory::Secrets && form.source_onepassword {
            vec![div()
                .flex()
                .flex_col()
                .gap(SPACE_1)
                .child(vault_ui::field_label(
                    t!("1Password reference"),
                    Some(t!("Resolved on demand on host")),
                    cx,
                ))
                .child(ui::Field::new(&form.op_ref).mono().small())
                .into_any_element()]
        } else {
            form.fields
                .iter()
                .enumerate()
                .map(|(ix, f)| {
                    let is_mono = f.sensitive
                        || f.kind.is_inherently_sensitive()
                        || matches!(
                            f.kind,
                            VaultFieldKind::CardNumber
                                | VaultFieldKind::Expiry
                                | VaultFieldKind::Cvc
                                | VaultFieldKind::Totp
                        );
                    let hint = if !f.required {
                        Some(t!("Optional"))
                    } else if f.kind == VaultFieldKind::Totp {
                        Some(t!("Codes generated on host"))
                    } else if f.kind == VaultFieldKind::Cvc {
                        Some(t!("Never exposed to agents"))
                    } else {
                        None
                    };
                    let eye = f.sensitive.then(|| {
                        let id = req_id.clone();
                        let masked = f.masked;
                        let bench = handle.clone();
                        ui::IconButton::new(
                            SharedString::from(format!("vreq-eye-{id}-{ix}")),
                            if masked { Icon::Eye } else { Icon::EyeOff },
                            if masked { t!("Show") } else { t!("Hide") },
                        )
                        .compact()
                        .on_click(move |_, window, cx| {
                            let _ = bench.update(cx, |bench, cx| {
                                if let Some(field) = bench
                                    .vault_cards
                                    .requests
                                    .get_mut(&id)
                                    .and_then(|form| form.fields.get_mut(ix))
                                {
                                    field.masked = !field.masked;
                                    let next = field.masked;
                                    field
                                        .input
                                        .update(cx, |i, cx| i.set_masked(next, window, cx));
                                }
                                bench.vault_cards.notify(cx);
                            });
                        })
                    });
                    let field = {
                        let base = ui::Field::new(&f.input).small();
                        if is_mono {
                            base.mono()
                        } else {
                            base
                        }
                    };
                    div()
                        .flex()
                        .flex_col()
                        .gap(SPACE_1)
                        .child(vault_ui::field_label(f.label.clone(), hint, cx))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(SPACE_1)
                                .child(div().flex_1().min_w_0().child(field))
                                .children(eye),
                        )
                        .into_any_element()
                })
                .collect()
        };

    let subtitle = req.reason.clone().unwrap_or_else(|| {
        t!(
            "{who} asked for this to fill the page blind. Entered values never enter the chat transcript.",
            who = who
        )
    });

    let body = div()
        .px(SPACE_4)
        .py(SPACE_3)
        .flex()
        .flex_col()
        .gap(SPACE_2)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(SPACE_0)
                .child(
                    div()
                        .text_size(theme::ui_text(TEXT_SM))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.text_primary)
                        .child(req.title.clone()),
                )
                .child(
                    div()
                        .text_size(theme::ui_text(TEXT_XS))
                        .text_color(t.text_tertiary)
                        .child(subtitle),
                ),
        )
        .children(source_toggle)
        .children(payment_preview)
        .children(doc_dropzone)
        .when(!inputs.is_empty(), |col| {
            col.child(div().flex().flex_col().gap(SPACE_2).children(inputs))
        })
        .children(form.error.clone().map(|err| {
            div()
                .text_size(theme::ui_text(TEXT_XS))
                .text_color(t.danger)
                .child(err)
        }))
        .child(vault_ui::whisper(
            t!(
                "Sealed locally · {who} only sees the vault reference, never raw values",
                who = who
            ),
            cx,
        ));

    let decline_id = req_id.clone();
    let once_id = req_id.clone();
    let save_id = req_id.clone();
    let bench_decline = handle.clone();
    let bench_once = handle.clone();
    let bench_save = handle;
    let save_label = if req.domain.is_some() {
        t!("Save to Vault & fill")
    } else {
        t!("Save to Vault")
    };
    let foot = div()
        .px(SPACE_4)
        .py(SPACE_2)
        .border_t(theme::border())
        .border_color(t.border)
        .bg(t.surface_sunken.opacity(0.45))
        .flex()
        .flex_wrap()
        .items_center()
        .justify_between()
        .gap(SPACE_2)
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_1H)
                .text_size(theme::ui_text(TEXT_2XS))
                .text_color(t.text_tertiary)
                .child(ui::icon(Icon::Lock, 11.).text_color(t.text_tertiary))
                .child(vault_ui::protection_rule(req.category)),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_1H)
                .child(
                    ui::Button::new(
                        SharedString::from(format!("vreq-decline-{decline_id}")),
                        t!("Decline"),
                        ui::Variant::Ghost,
                    )
                    .disabled(form.busy)
                    .on_click(move |_, _, cx| {
                        let _ = bench_decline.update(cx, |bench, cx| {
                            cancel_request_card(bench, decline_id.clone(), cx)
                        });
                    }),
                )
                .child(
                    ui::Button::new(
                        SharedString::from(format!("vreq-once-{once_id}")),
                        t!("Use once"),
                        ui::Variant::Outline,
                    )
                    .disabled(form.busy)
                    .on_click(move |_, _, cx| {
                        let _ = bench_once.update(cx, |bench, cx| {
                            submit_request_card(bench, once_id.clone(), true, cx)
                        });
                    }),
                )
                .child(
                    ui::Button::new(
                        SharedString::from(format!("vreq-save-{save_id}")),
                        save_label,
                        ui::Variant::Primary,
                    )
                    .icon(Icon::ShieldCheck)
                    .disabled(form.busy)
                    .on_click(move |_, _, cx| {
                        let _ = bench_save.update(cx, |bench, cx| {
                            submit_request_card(bench, save_id.clone(), false, cx)
                        });
                    }),
                ),
        );

    let card = div()
        .w_full()
        .rounded(theme::radius_3())
        .overflow_hidden()
        .border(theme::border())
        .border_color(t.border_strong)
        .bg(t.surface_raised)
        .shadow(theme::shadow_large())
        .flex()
        .flex_col()
        .child(bar)
        .child(body)
        .child(foot);

    card_frame(card, cx)
}

fn render_approval_card(
    app: &VaultPendingApproval,
    form: &ApprovalForm,
    handle: WeakEntity<Workbench>,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let color = vault_ui::category_color(app.category, &t);
    let who = vault_ui::format_actor(&app.agent_name);
    let app_id = app.id.clone();
    let domain = if app.domain.is_empty() {
        t!("this page").to_owned()
    } else {
        app.domain.clone()
    };

    let bar = div()
        .px(SPACE_3)
        .py(SPACE_2)
        .border_b(theme::border())
        .border_color(t.border)
        .bg(color.opacity(0.08))
        .flex()
        .items_center()
        .gap(SPACE_2)
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_1H)
                .text_size(theme::ui_text(TEXT_2XS))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.text_primary)
                .child(ui::icon(vault_ui::category_icon(app.category), 12.).text_color(color))
                .child(t!("Vault Fill Approval")),
        )
        .child(vault_ui::pill(
            vault_ui::category_label(app.category),
            color,
            None,
            cx,
        ))
        .child(div().flex_1())
        .child(vault_ui::domain_chip(&domain, cx));

    let masked_hint = app.masked_summary.as_deref().unwrap_or("••••");
    let card_badge = (app.category == VaultCategory::Payments).then(|| {
        vault_ui::dark_glass_card(
            CardBrand::detect(masked_hint, &app.entry_title),
            &vault_ui::mask_card_last4(masked_hint),
            &app.entry_title,
            "••/••",
            Some(&domain),
            cx,
        )
    });

    let cvc_row = form.cvc.as_ref().map(|input| {
        div()
            .flex()
            .flex_col()
            .gap(SPACE_1)
            .child(vault_ui::field_label(
                t!("CVC / CVV"),
                Some(t!("Used once for this fill · never saved")),
                cx,
            ))
            .child(ui::Field::new(input).mono().small())
    });

    let remember_row =
        (app.category != VaultCategory::Payments && !app.domain.is_empty()).then(|| {
            let id = app_id.clone();
            let bench = handle.clone();
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(SPACE_2)
                .child(
                    div()
                        .text_size(theme::ui_text(TEXT_XS))
                        .text_color(t.text_secondary)
                        .child(t!("Always allow on {domain}", domain = domain.clone())),
                )
                .child(
                    Switch::new(SharedString::from(format!("vapp-rem-{id}")))
                        .checked(form.remember_domain)
                        .on_change(move |on: &bool, _, cx| {
                            let on = *on;
                            let _ = bench.update(cx, |bench, cx| {
                                if let Some(form) = bench.vault_cards.approvals.get_mut(&id) {
                                    form.remember_domain = on;
                                }
                                bench.vault_cards.notify(cx);
                            });
                        }),
                )
        });

    let fields_line = (!app.field_keys.is_empty()).then(|| {
        div()
            .text_size(theme::ui_text(TEXT_2XS))
            .font_family(theme::mono_font())
            .text_color(t.text_tertiary)
            .child(t!("Fields: {fields}", fields = app.field_keys.join(", ")))
    });

    let body = div()
        .px(SPACE_4)
        .py(SPACE_3)
        .flex()
        .flex_col()
        .gap(SPACE_2)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(SPACE_0)
                .child(
                    div()
                        .text_size(theme::ui_text(TEXT_SM))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.text_primary)
                        .child(app.entry_title.clone()),
                )
                .child(
                    div()
                        .text_size(theme::ui_text(TEXT_XS))
                        .text_color(t.text_secondary)
                        .child(app.reason.clone()),
                )
                .children(fields_line),
        )
        .children(card_badge)
        .children(cvc_row)
        .children(remember_row)
        .children(form.error.clone().map(|err| {
            div()
                .text_size(theme::ui_text(TEXT_XS))
                .text_color(t.danger)
                .child(err)
        }))
        .child(vault_ui::whisper(
            t!(
                "Host injects directly into {domain} · {who} never sees the secret",
                domain = domain,
                who = who
            ),
            cx,
        ));

    let deny_id = app_id.clone();
    let approve_id = app_id.clone();
    let bench_deny = handle.clone();
    let bench_approve = handle;
    let approve_label = if app.requires_biometric {
        t!("Approve with Touch ID")
    } else {
        t!("Approve blind-fill")
    };
    let approve_icon = if app.requires_biometric {
        Icon::Fingerprint
    } else {
        Icon::ShieldCheck
    };

    let foot = div()
        .px(SPACE_4)
        .py(SPACE_2)
        .border_t(theme::border())
        .border_color(t.border)
        .bg(t.surface_sunken.opacity(0.45))
        .flex()
        .items_center()
        .justify_between()
        .gap(SPACE_2)
        .child(
            div()
                .text_size(theme::ui_text(TEXT_2XS))
                .text_color(t.text_tertiary)
                .child(t!("Requested by {who}", who = who)),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_1H)
                .child(
                    ui::Button::new(
                        SharedString::from(format!("vapp-deny-{deny_id}")),
                        t!("Deny"),
                        ui::Variant::Ghost,
                    )
                    .disabled(form.busy)
                    .on_click(move |_, _, cx| {
                        let _ = bench_deny.update(cx, |bench, cx| {
                            answer_approval_card(bench, deny_id.clone(), false, cx)
                        });
                    }),
                )
                .child(
                    ui::Button::new(
                        SharedString::from(format!("vapp-ok-{approve_id}")),
                        approve_label,
                        ui::Variant::Primary,
                    )
                    .icon(approve_icon)
                    .disabled(form.busy)
                    .on_click(move |_, _, cx| {
                        let _ = bench_approve.update(cx, |bench, cx| {
                            answer_approval_card(bench, approve_id.clone(), true, cx)
                        });
                    }),
                ),
        );

    let card = div()
        .w_full()
        .rounded(theme::radius_3())
        .overflow_hidden()
        .border(theme::border())
        .border_color(color.opacity(0.45))
        .bg(t.surface_raised)
        .shadow(theme::shadow_large())
        .flex()
        .flex_col()
        .child(bar)
        .child(body)
        .child(foot);

    card_frame(card, cx)
}
