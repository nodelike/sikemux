//! Settings → Vault: encrypted storage for Details, Secrets (native + 1Password op://),
//! Payments, encrypted Documents, and the blind-fill Activity Log.

use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::component::input::InputState;
use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, AppContext, ClipboardItem, Context, Entity, Focusable, FontWeight,
    InteractiveElement, IntoElement, ParentElement, PathPromptOptions, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Window,
};
use sikemux_calls::call;
use sikemux_calls::vault::{
    VaultAuditEntry, VaultAuditLogParams, VaultCategory, VaultDeleteParams, VaultDocumentInput,
    VaultDocumentSummary, VaultEntryDetail, VaultEntrySummary, VaultFieldInput, VaultFieldKind,
    VaultGetParams, VaultListParams, VaultScope, VaultSource, VaultStageDocumentParams,
    VaultUpsertParams,
};
use sikemux_corekit::accounts;

use crate::core::Target;
use crate::icons::Icon;
use crate::settings::index::Page::Vault;
use crate::settings::index::{row, section, Entry};
use crate::settings::rows;
use crate::theme::{
    self, Tokens, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_3, SPACE_4, SPACE_5, TEXT_2XS,
    TEXT_3XS, TEXT_SM, TEXT_XS,
};
use crate::toast;
use crate::ui::time::ago;
use crate::ui::{self, dialog, Clicks};
use crate::vault_ui::{self, CardBrand};
use crate::workbench::Workbench;

pub(in crate::settings) const ENTRIES: &[Entry] = &[
    section(
        Vault,
        "Vault",
        "vault autofill blind fill encrypted personal details secrets passwords payments cards documents audit log 1password op",
    ),
    row(
        Vault,
        "Vault",
        "Details",
        "profile name address phone company legal entity gst pan tax vat billing shipping",
    ),
    row(
        Vault,
        "Vault",
        "Secrets",
        "secrets api key token env login password 2fa totp 1password op://",
    ),
    row(
        Vault,
        "Vault",
        "Payments",
        "payments credit card debit card visa mastercard amex cvc touch id biometric checkout",
    ),
    row(
        Vault,
        "Vault",
        "Docs",
        "documents pdf passport pan aadhaar rent agreement bill upload encrypted",
    ),
    row(
        Vault,
        "Vault",
        "Activity",
        "activity audit log blind fill upload history approvals",
    ),
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Tab {
    #[default]
    Details,
    Secrets,
    Payments,
    Docs,
    Activity,
}

impl Tab {
    const ALL: [Tab; 5] = [
        Tab::Details,
        Tab::Secrets,
        Tab::Payments,
        Tab::Docs,
        Tab::Activity,
    ];

    fn category(self) -> Option<VaultCategory> {
        match self {
            Tab::Details => Some(VaultCategory::Details),
            Tab::Secrets => Some(VaultCategory::Secrets),
            Tab::Payments => Some(VaultCategory::Payments),
            Tab::Docs => Some(VaultCategory::Documents),
            Tab::Activity => None,
        }
    }

    fn index(self) -> usize {
        match self {
            Tab::Details => 0,
            Tab::Secrets => 1,
            Tab::Payments => 2,
            Tab::Docs => 3,
            Tab::Activity => 4,
        }
    }

    fn from_index(ix: usize) -> Self {
        Self::ALL.get(ix).copied().unwrap_or(Tab::Details)
    }

    fn add_label(self) -> &'static str {
        match self {
            Tab::Details => t!("Add detail"),
            Tab::Secrets => t!("Add secret"),
            Tab::Payments => t!("Add card"),
            Tab::Docs => t!("Add document"),
            Tab::Activity => t!("Refresh"),
        }
    }
}

pub struct Fields {
    pub(super) stale: bool,
    loading: bool,
    tab: Tab,
    scope: VaultScope,
    search: Entity<InputState>,
    _typing: Subscription,
    entries: Vec<VaultEntrySummary>,
    audit: Vec<VaultAuditEntry>,
    selected: Option<String>,
    revealed: Option<VaultEntryDetail>,
    revealing: bool,
    editor: Option<EditorState>,
    confirm_delete: Option<(String, String)>,
}

struct EditorField {
    key: String,
    label_input: Entity<InputState>,
    value_input: Entity<InputState>,
    kind: VaultFieldKind,
    sensitive: bool,
    masked: bool,
    custom: bool,
}

struct EditorState {
    id: Option<String>,
    category: VaultCategory,
    scope: VaultScope,
    source_onepassword: bool,
    require_approval: bool,
    title: Entity<InputState>,
    domains: Entity<InputState>,
    op_ref: Entity<InputState>,
    fields: Vec<EditorField>,
    doc_path: Option<PathBuf>,
    doc_existing: Option<VaultDocumentSummary>,
    error: Option<SharedString>,
    saving: bool,
    _subs: Vec<Subscription>,
}

fn change_fields<R>(
    bench: &Workbench,
    cx: &mut App,
    change: impl FnOnce(&mut Fields, &mut Context<crate::settings::Settings>) -> R,
) -> Option<R> {
    super::change_form(bench, cx, |pages| pages.vault.as_mut(), change)
}

pub(super) fn make(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    let search = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Search vault…")));
    let typing = super::follow_typing(bench, &search, cx);
    super::change_pages(bench, cx, |pages, _| {
        pages.vault = Some(Fields {
            stale: false,
            loading: true,
            tab: Tab::Details,
            scope: VaultScope::Personal,
            search,
            _typing: typing,
            entries: Vec::new(),
            audit: Vec::new(),
            selected: None,
            revealed: None,
            revealing: false,
            editor: None,
            confirm_delete: None,
        });
    });
    read(bench, window, cx);
}

pub(super) fn read(bench: &mut Workbench, _window: &mut Window, cx: &mut Context<Workbench>) {
    change_fields(bench, cx, |fields, _| {
        fields.stale = false;
        fields.loading = true;
    });
    let list_req = Target::here(&bench.asks)
        .request_typed::<call::VaultList>(Some(VaultListParams::default()));
    let audit_req =
        Target::here(&bench.asks).request_typed::<call::VaultAuditLog>(Some(VaultAuditLogParams {
            entry_id: None,
            limit: Some(100),
        }));
    cx.spawn(async move |bench, cx| {
        let entries = list_req.await;
        let audit = audit_req.await;
        let _ = bench.update(cx, |bench, cx| {
            change_fields(bench, cx, |fields, _| {
                fields.loading = false;
                if let Ok(entries) = entries {
                    if let Some(sel) = &fields.selected {
                        if !entries.iter().any(|e| &e.id == sel) {
                            fields.selected = None;
                            fields.revealed = None;
                        }
                    }
                    fields.entries = entries;
                }
                if let Ok(audit) = audit {
                    fields.audit = audit;
                }
            });
        });
    })
    .detach();
}

fn toggle_reveal(bench: &mut Workbench, id: String, cx: &mut Context<Workbench>) {
    let already_revealed = super::state(bench, cx)
        .and_then(|pages| pages.vault.as_ref())
        .and_then(|fields| fields.revealed.as_ref())
        .is_some_and(|detail| detail.summary.id == id && detail.revealed);
    if already_revealed {
        change_fields(bench, cx, |fields, _| {
            fields.revealed = None;
            fields.revealing = false;
        });
        return;
    }
    change_fields(bench, cx, |fields, _| fields.revealing = true);
    let asked = Target::here(&bench.asks).request_typed::<call::VaultGet>(VaultGetParams {
        id: id.clone(),
        reveal: true,
        resolve_external: false,
    });
    cx.spawn(async move |bench, cx| {
        let answer = asked.await;
        let _ = bench.update(cx, |bench, cx| match answer {
            Ok(detail) => {
                change_fields(bench, cx, |fields, _| {
                    fields.revealing = false;
                    fields.revealed = Some(detail);
                });
            }
            Err(error) => {
                change_fields(bench, cx, |fields, _| fields.revealing = false);
                bench.notify(toast::error(error), cx);
            }
        });
    })
    .detach();
}

fn open_document(bench: &mut Workbench, id: String, reveal: bool, cx: &mut Context<Workbench>) {
    let asked = Target::here(&bench.asks)
        .request_typed::<call::VaultStageDocument>(VaultStageDocumentParams { id });
    cx.spawn(async move |bench, cx| {
        let answer = asked.await;
        let _ = bench.update(cx, |bench, cx| match answer {
            Ok(staged) => {
                let path = PathBuf::from(&staged.path);
                if reveal {
                    cx.reveal_path(&path);
                    bench.notify(
                        toast::info(t!(
                            "Revealed {name} in private staging",
                            name = staged.filename
                        )),
                        cx,
                    );
                } else {
                    cx.open_with_system(&path);
                    bench.notify(
                        toast::info(t!("Opened {name} from Vault", name = staged.filename)),
                        cx,
                    );
                }
            }
            Err(error) => bench.notify(toast::error(error), cx),
        });
    })
    .detach();
}

fn delete_entry(bench: &mut Workbench, id: String, title: String, cx: &mut Context<Workbench>) {
    change_fields(bench, cx, |fields, _| fields.confirm_delete = None);
    let asked = Target::here(&bench.asks)
        .request_typed::<call::VaultDelete>(VaultDeleteParams { id: id.clone() });
    cx.spawn(async move |bench, cx| {
        let answer = asked.await;
        let _ = bench.update(cx, |bench, cx| match answer {
            Ok(()) => {
                change_fields(bench, cx, |fields, _| {
                    fields.entries.retain(|entry| entry.id != id);
                    if fields.selected.as_deref() == Some(id.as_str()) {
                        fields.selected = None;
                        fields.revealed = None;
                    }
                    fields.stale = true;
                });
                bench.notify(
                    toast::success(t!("Removed “{title}” from Vault", title = title)),
                    cx,
                );
            }
            Err(error) => bench.notify(toast::error(error), cx),
        });
    })
    .detach();
}

struct FieldSpec<'a> {
    key: &'a str,
    label: &'a str,
    placeholder: &'a str,
    initial: Option<&'a str>,
    kind: VaultFieldKind,
    sensitive: bool,
    custom: bool,
}

fn make_editor_field(
    bench: &Workbench,
    spec: FieldSpec<'_>,
    window: &mut Window,
    cx: &mut Context<Workbench>,
    subs: &mut Vec<Subscription>,
) -> EditorField {
    let label_val = spec.label.to_owned();
    let label_input = cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder(t!("Field name"))
            .default_value(label_val)
    });
    let masked = spec.sensitive || spec.kind.is_inherently_sensitive();
    let ph = spec.placeholder.to_owned();
    let init = spec.initial.unwrap_or_default().to_owned();
    let value_input = cx.new(|cx| {
        let mut s = InputState::new(window, cx).placeholder(ph);
        if masked {
            s = s.masked(true);
        }
        if !init.is_empty() {
            s = s.default_value(init);
        }
        s
    });
    subs.push(super::follow_typing(bench, &label_input, cx));
    subs.push(super::follow_typing(bench, &value_input, cx));
    EditorField {
        key: spec.key.to_owned(),
        label_input,
        value_input,
        kind: spec.kind,
        sensitive: spec.sensitive,
        masked,
        custom: spec.custom,
    }
}

fn default_editor_fields(
    bench: &Workbench,
    category: VaultCategory,
    window: &mut Window,
    cx: &mut Context<Workbench>,
    subs: &mut Vec<Subscription>,
) -> Vec<EditorField> {
    let mut out = Vec::new();
    let specs: &[(&str, &str, &str, VaultFieldKind, bool)] = match category {
        VaultCategory::Details => &[
            (
                "full_name",
                "Full name",
                "Alex Rivera",
                VaultFieldKind::Text,
                false,
            ),
            (
                "email",
                "Email",
                "you@domain.com",
                VaultFieldKind::Email,
                false,
            ),
            (
                "phone",
                "Phone",
                "+1 415 555 0194",
                VaultFieldKind::Phone,
                false,
            ),
            (
                "address",
                "Street address",
                "548 Market St, Suite 410",
                VaultFieldKind::Address,
                false,
            ),
            (
                "city_postal",
                "City & postal",
                "San Francisco, CA 94104",
                VaultFieldKind::Text,
                false,
            ),
            (
                "company",
                "Legal entity / Company",
                "Acme Studio Inc.",
                VaultFieldKind::Text,
                false,
            ),
            (
                "tax_id",
                "Tax / GST / PAN ID",
                "Optional",
                VaultFieldKind::TaxId,
                true,
            ),
        ],
        VaultCategory::Secrets => &[
            (
                "username",
                "Username or email",
                "Optional",
                VaultFieldKind::Email,
                false,
            ),
            (
                "secret",
                "Password / API secret",
                "Paste secret value",
                VaultFieldKind::Password,
                true,
            ),
            (
                "totp",
                "2FA / TOTP setup key",
                "Optional · base32 seed",
                VaultFieldKind::Totp,
                true,
            ),
        ],
        VaultCategory::Payments => &[
            (
                "cardholder",
                "Name on card",
                "ALEX RIVERA",
                VaultFieldKind::Text,
                false,
            ),
            (
                "card_number",
                "Card number",
                "4242 4242 4242 4242",
                VaultFieldKind::CardNumber,
                true,
            ),
            ("expiry", "Expiry", "MM/YY", VaultFieldKind::Expiry, true),
            (
                "cvc",
                "CVC / CVV",
                "3 or 4 digits",
                VaultFieldKind::Cvc,
                true,
            ),
            (
                "billing_address",
                "Billing address",
                "Optional",
                VaultFieldKind::Address,
                false,
            ),
        ],
        VaultCategory::Documents => &[(
            "document_number",
            "ID / Document number",
            "Optional (e.g. PAN / Passport)",
            VaultFieldKind::TaxId,
            true,
        )],
    };
    for (key, label, ph, kind, sensitive) in specs {
        out.push(make_editor_field(
            bench,
            FieldSpec {
                key,
                label,
                placeholder: ph,
                initial: None,
                kind: *kind,
                sensitive: *sensitive,
                custom: false,
            },
            window,
            cx,
            subs,
        ));
    }
    out
}

fn open_add_editor(
    bench: &mut Workbench,
    category: VaultCategory,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    let scope = super::state(bench, cx)
        .and_then(|pages| pages.vault.as_ref())
        .map(|f| f.scope.clone())
        .unwrap_or_default();
    let title_ph = match category {
        VaultCategory::Details => t!("e.g. Personal Profile or Company Billing"),
        VaultCategory::Secrets => t!("e.g. Stripe Live Key or Cloud Portal"),
        VaultCategory::Payments => t!("e.g. Chase Sapphire Visa"),
        VaultCategory::Documents => t!("e.g. PAN Card or Rent Agreement"),
    };
    let mut subs = Vec::new();
    let title = cx.new(|cx| InputState::new(window, cx).placeholder(title_ph));
    let domains = cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder(t!("Optional · e.g. portal.example.com, github.com"))
    });
    let op_ref =
        cx.new(|cx| InputState::new(window, cx).placeholder("op://Personal/Item/credential"));
    subs.push(super::follow_typing(bench, &title, cx));
    subs.push(super::follow_typing(bench, &domains, cx));
    subs.push(super::follow_typing(bench, &op_ref, cx));
    let fields = default_editor_fields(bench, category, window, cx, &mut subs);
    title.update(cx, |input, cx| input.focus(window, cx));
    let editor = EditorState {
        id: None,
        category,
        scope,
        source_onepassword: false,
        require_approval: matches!(
            category,
            VaultCategory::Secrets | VaultCategory::Payments | VaultCategory::Documents
        ),
        title,
        domains,
        op_ref,
        fields,
        doc_path: None,
        doc_existing: None,
        error: None,
        saving: false,
        _subs: subs,
    };
    change_fields(bench, cx, |fields, _| fields.editor = Some(editor));
}

fn open_edit_editor(
    bench: &mut Workbench,
    id: String,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    let asked = Target::here(&bench.asks).request_typed::<call::VaultGet>(VaultGetParams {
        id,
        reveal: true,
        resolve_external: false,
    });
    cx.spawn_in(window, async move |bench, cx| {
        let answer = asked.await;
        let _ = bench.update_in(cx, |bench, window, cx| match answer {
            Ok(detail) => {
                let summary = detail.summary;
                let mut subs = Vec::new();
                let title_val = summary.title.clone();
                let title = cx.new(|cx| InputState::new(window, cx).default_value(title_val));
                let domains_val = summary.domains.join(", ");
                let domains = cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder(t!("Optional · comma-separated domains"))
                        .default_value(domains_val)
                });
                let (source_onepassword, op_ref_val) = match &summary.source {
                    VaultSource::OnePassword { reference } => (true, reference.clone()),
                    VaultSource::Sikemux => (false, String::new()),
                };
                let op_ref = cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder("op://Personal/Item/credential")
                        .default_value(op_ref_val)
                });
                subs.push(super::follow_typing(bench, &title, cx));
                subs.push(super::follow_typing(bench, &domains, cx));
                subs.push(super::follow_typing(bench, &op_ref, cx));
                let mut fields: Vec<EditorField> = detail
                    .fields
                    .iter()
                    .map(|f| {
                        make_editor_field(
                            bench,
                            FieldSpec {
                                key: &f.key,
                                label: &f.label,
                                placeholder: "",
                                initial: f.value.as_deref(),
                                kind: f.kind,
                                sensitive: f.sensitive,
                                custom: true,
                            },
                            window,
                            cx,
                            &mut subs,
                        )
                    })
                    .collect();
                if fields.is_empty() && summary.category != VaultCategory::Documents {
                    fields = default_editor_fields(bench, summary.category, window, cx, &mut subs);
                }
                let editor = EditorState {
                    id: Some(summary.id),
                    category: summary.category,
                    scope: summary.scope,
                    source_onepassword,
                    require_approval: summary.require_approval,
                    title,
                    domains,
                    op_ref,
                    fields,
                    doc_path: None,
                    doc_existing: summary.document,
                    error: None,
                    saving: false,
                    _subs: subs,
                };
                change_fields(bench, cx, |fields, _| fields.editor = Some(editor));
            }
            Err(error) => bench.notify(toast::error(error), cx),
        });
    })
    .detach();
}

fn add_custom_editor_field(
    bench: &mut Workbench,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    let Some(category) = super::state(bench, cx)
        .and_then(|pages| pages.vault.as_ref())
        .and_then(|fields| fields.editor.as_ref())
        .map(|editor| editor.category)
    else {
        return;
    };
    let mut subs = Vec::new();
    let sensitive = matches!(category, VaultCategory::Secrets | VaultCategory::Payments);
    let kind = if sensitive {
        VaultFieldKind::Secret
    } else {
        VaultFieldKind::Text
    };
    let idx = super::state(bench, cx)
        .and_then(|pages| pages.vault.as_ref())
        .and_then(|fields| fields.editor.as_ref())
        .map_or(1, |editor| editor.fields.len() + 1);
    let key = format!("field_{idx}");
    let field = make_editor_field(
        bench,
        FieldSpec {
            key: &key,
            label: "",
            placeholder: t!("Value"),
            initial: None,
            kind,
            sensitive,
            custom: true,
        },
        window,
        cx,
        &mut subs,
    );
    change_fields(bench, cx, |fields, _| {
        if let Some(editor) = fields.editor.as_mut() {
            editor.fields.push(field);
            editor._subs.extend(subs);
        }
    });
}

fn pick_editor_document(_bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    let picked = cx.prompt_for_paths(PathPromptOptions {
        files: true,
        directories: false,
        multiple: false,
        prompt: Some(t!("Choose a document to encrypt in Vault").into()),
    });
    cx.spawn_in(window, async move |bench, cx| {
        if let Ok(Ok(Some(paths))) = picked.await {
            if let Some(path) = paths.into_iter().next() {
                let _ = bench.update_in(cx, |bench, window, cx| {
                    let filename = path
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    change_fields(bench, cx, |fields, cx| {
                        if let Some(editor) = fields.editor.as_mut() {
                            if editor.title.read(cx).value().trim().is_empty()
                                && !filename.is_empty()
                            {
                                editor
                                    .title
                                    .update(cx, |input, cx| input.set_value(filename, window, cx));
                            }
                            editor.doc_path = Some(path);
                            editor.error = None;
                        }
                    });
                });
            }
        }
    })
    .detach();
}

fn slugify_key(label: &str, fallback: &str) -> String {
    let mut key = String::new();
    let mut last_sep = false;
    for ch in label.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            key.push(ch.to_ascii_lowercase());
            last_sep = false;
        } else if !key.is_empty() && !last_sep {
            key.push('_');
            last_sep = true;
        }
    }
    let key = key.trim_matches('_').to_owned();
    if key.is_empty() {
        fallback.to_owned()
    } else {
        key
    }
}

fn save_editor(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    let built: Result<VaultUpsertParams, String> = (|| {
        let pages = super::state(bench, cx).ok_or_else(|| "Settings closed".to_owned())?;
        let fields = pages
            .vault
            .as_ref()
            .ok_or_else(|| "Vault not open".to_owned())?;
        let editor = fields
            .editor
            .as_ref()
            .ok_or_else(|| "No editor open".to_owned())?;
        let title = editor.title.read(cx).value().trim().to_owned();
        if title.is_empty() {
            return Err(t!("Enter a label for this item.").to_owned());
        }
        let domains: Vec<String> = editor
            .domains
            .read(cx)
            .value()
            .split([',', '\n', ' '])
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .map(str::to_owned)
            .collect();
        let source = if editor.category == VaultCategory::Secrets && editor.source_onepassword {
            let reference = editor.op_ref.read(cx).value().trim().to_owned();
            if !reference.starts_with("op://") {
                return Err(t!("1Password reference must start with op://").to_owned());
            }
            VaultSource::OnePassword { reference }
        } else {
            VaultSource::Sikemux
        };
        let document = if let Some(path) = &editor.doc_path {
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
        if editor.category == VaultCategory::Documents
            && document.is_none()
            && editor.doc_existing.is_none()
        {
            return Err(t!("Choose a file to attach to this document.").to_owned());
        }
        let mut field_inputs = Vec::new();
        if let VaultSource::OnePassword { ref reference } = source {
            field_inputs.push(VaultFieldInput {
                key: "secret".to_owned(),
                label: "1Password reference".to_owned(),
                kind: VaultFieldKind::Secret,
                sensitive: true,
                value: None,
                op_ref: Some(reference.clone()),
            });
        } else {
            for (ix, f) in editor.fields.iter().enumerate() {
                let raw_val = f.value_input.read(cx).value().trim().to_owned();
                let raw_lbl = f.label_input.read(cx).value().trim().to_owned();
                if raw_val.is_empty() {
                    continue;
                }
                let key = if f.custom {
                    slugify_key(&raw_lbl, &format!("field_{}", ix + 1))
                } else {
                    f.key.clone()
                };
                let label = if raw_lbl.is_empty() {
                    key.clone()
                } else {
                    raw_lbl
                };
                field_inputs.push(VaultFieldInput {
                    key,
                    label,
                    kind: f.kind,
                    sensitive: f.sensitive,
                    value: Some(raw_val),
                    op_ref: None,
                });
            }
        }
        Ok(VaultUpsertParams {
            id: editor.id.clone(),
            title,
            category: editor.category,
            scope: editor.scope.clone(),
            source,
            delivery_mode: None,
            domains,
            require_approval: editor.require_approval,
            fields: field_inputs,
            document,
            subtitle: None,
        })
    })();

    let params = match built {
        Ok(params) => params,
        Err(message) => {
            change_fields(bench, cx, |fields, _| {
                if let Some(editor) = fields.editor.as_mut() {
                    editor.error = Some(message.into());
                }
            });
            return;
        }
    };

    let title = params.title.clone();
    let is_update = params.id.is_some();
    change_fields(bench, cx, |fields, _| {
        if let Some(editor) = fields.editor.as_mut() {
            editor.saving = true;
            editor.error = None;
        }
    });
    let asked = Target::here(&bench.asks).request_typed::<call::VaultUpsert>(params);
    cx.spawn(async move |bench, cx| {
        let answer = asked.await;
        let _ = bench.update(cx, |bench, cx| match answer {
            Ok(saved) => {
                change_fields(bench, cx, |fields, _| {
                    fields.editor = None;
                    fields.selected = Some(saved.id.clone());
                    fields.revealed = None;
                    fields.stale = true;
                });
                let msg = if is_update {
                    t!("Updated “{title}” in Vault", title = title)
                } else {
                    t!("Saved “{title}” to Vault", title = title)
                };
                bench.notify(toast::success(msg), cx);
            }
            Err(error) => {
                change_fields(bench, cx, |fields, _| {
                    if let Some(editor) = fields.editor.as_mut() {
                        editor.saving = false;
                        editor.error = Some(error.into());
                    }
                });
            }
        });
    })
    .detach();
}

pub(super) fn render(
    bench: &Workbench,
    pages: &super::State,
    lit: &dyn Fn(&str) -> bool,
    this: &Entity<Workbench>,
    cx: &App,
) -> Vec<AnyElement> {
    let Some(fields) = pages.vault.as_ref() else {
        return vec![rows::section(
            n!("Vault"),
            None,
            None,
            lit("Vault"),
            rows::note(t!("Loading Vault…"), cx),
            cx,
        )];
    };

    let t = Tokens::get(cx).clone();
    let account = bench.this_view().and_then(|view| view.account.as_deref());
    let orgs: Vec<_> = accounts::shown_orgs(account).cloned().collect();

    let scope_matches = |entry: &VaultEntrySummary| entry.scope.key() == fields.scope.key();
    let count_of = |cat: VaultCategory| {
        fields
            .entries
            .iter()
            .filter(|e| scope_matches(e) && e.category == cat)
            .count() as u64
    };

    let tab_options = [
        (
            Some(Icon::User),
            SharedString::from(t!("Details")),
            Some(count_of(VaultCategory::Details)),
        ),
        (
            Some(Icon::Key),
            SharedString::from(t!("Secrets")),
            Some(count_of(VaultCategory::Secrets)),
        ),
        (
            Some(Icon::CreditCard),
            SharedString::from(t!("Payments")),
            Some(count_of(VaultCategory::Payments)),
        ),
        (
            Some(Icon::File),
            SharedString::from(t!("Docs")),
            Some(count_of(VaultCategory::Documents)),
        ),
        (
            Some(Icon::ShieldCheck),
            SharedString::from(t!("Activity")),
            Some(fields.audit.len() as u64),
        ),
    ];
    let mut seg = ui::Segmented::words(
        "vault-tabs",
        tab_options
            .iter()
            .map(|(_, label, count)| (label.clone(), *count))
            .collect(),
        fields.tab.index(),
    )
    .on_select({
        let this = this.clone();
        move |ix, _, cx| {
            let tab = Tab::from_index(*ix);
            this.update(cx, |bench, cx| {
                change_fields(bench, cx, |fields, _| {
                    fields.tab = tab;
                    fields.selected = None;
                    fields.revealed = None;
                });
            });
        }
    });
    for (ix, (glyph, _, _)) in tab_options.iter().enumerate() {
        if let Some(glyph) = glyph {
            seg = seg.glyph(ix, *glyph);
        }
    }

    let scope_bar = (!orgs.is_empty()).then(|| {
        let mut scopes = vec![(SharedString::from(t!("Personal")), VaultScope::Personal)];
        for org in &orgs {
            scopes.push((
                SharedString::from(org.name.clone()),
                VaultScope::Org {
                    org_id: org.id.clone(),
                    org_name: Some(org.name.clone()),
                },
            ));
        }
        let selected_ix = scopes
            .iter()
            .position(|(_, s)| s.key() == fields.scope.key())
            .unwrap_or(0);
        let words: Vec<(SharedString, Option<u64>)> = scopes
            .iter()
            .map(|(name, _)| (name.clone(), None))
            .collect();
        let this = this.clone();
        ui::Segmented::words("vault-scope", words, selected_ix).on_select(move |ix, _, cx| {
            if let Some((_, next_scope)) = scopes.get(*ix).cloned() {
                this.update(cx, |bench, cx| {
                    change_fields(bench, cx, |fields, _| {
                        fields.scope = next_scope;
                        fields.selected = None;
                        fields.revealed = None;
                    });
                });
            }
        })
    });

    let make_add_button = || {
        let this = this.clone();
        let tab = fields.tab;
        match tab.category() {
            Some(category) => ui::Button::new("vault-add", tab.add_label(), ui::Variant::Primary)
                .icon(Icon::Plus)
                .on_click(move |_, window, cx| {
                    this.update(cx, |bench, cx| open_add_editor(bench, category, window, cx))
                }),
            None => ui::Button::new("vault-refresh", tab.add_label(), ui::Variant::Outline)
                .icon(Icon::Refresh)
                .on_click(move |_, window, cx| {
                    this.update(cx, |bench, cx| read(bench, window, cx))
                }),
        }
    };

    let controls = div()
        .px(SPACE_4)
        .py(SPACE_3)
        .border_b(theme::border())
        .border_color(t.line_soft)
        .flex()
        .flex_col()
        .gap(SPACE_2)
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap(SPACE_2)
                .child(seg)
                .children(scope_bar),
        )
        .when(fields.tab != Tab::Activity, |bar| {
            bar.child(
                div()
                    .flex()
                    .items_center()
                    .gap(SPACE_2)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(ui::Field::new(&fields.search).small().icon(Icon::Search)),
                    )
                    .child(make_add_button()),
            )
        })
        .when(fields.tab == Tab::Activity, |bar| {
            bar.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(SPACE_2)
                    .child(
                        div()
                            .text_size(theme::ui_text(TEXT_XS))
                            .text_color(t.text_tertiary)
                            .child(t!("Metadata-only log of zero-exposure blind-fills, blind-uploads, and approvals.")),
                    )
                    .child(make_add_button()),
            )
        });

    let tab_banner: Option<AnyElement> = match fields.tab {
        Tab::Details => Some(
            div()
                .px(SPACE_4)
                .py(SPACE_2)
                .border_b(theme::border())
                .border_color(t.line_soft)
                .flex()
                .items_center()
                .gap(SPACE_2)
                .child(vault_ui::pill(
                    vault_ui::protection_rule(VaultCategory::Details),
                    t.live,
                    Some(Icon::Check),
                    cx,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(theme::ui_text(TEXT_2XS))
                        .text_color(t.text_tertiary)
                        .child(t!("Profiles, addresses, legal entity, GST/PAN and custom key-value pairs.")),
                )
                .into_any_element(),
        ),
        Tab::Secrets => Some(
            div()
                .px(SPACE_4)
                .py(SPACE_2)
                .border_b(theme::border())
                .border_color(t.line_soft)
                .flex()
                .flex_col()
                .gap(SPACE_2)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(SPACE_2)
                        .child(vault_ui::pill(
                            vault_ui::protection_rule(VaultCategory::Secrets),
                            t.warn,
                            Some(Icon::Lock),
                            cx,
                        ))
                        .child(vault_ui::onepassword_badge(None, cx))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(theme::ui_text(TEXT_2XS))
                                .text_color(t.text_tertiary)
                                .child(t!("Store encrypted in Sikemux Vault or link an op:// reference from 1Password.")),
                        ),
                )
                .child(vault_ui::honest_privacy_badge(&VaultSource::Sikemux, cx))
                .into_any_element(),
        ),
        Tab::Payments => Some(
            div()
                .px(SPACE_4)
                .py(SPACE_2)
                .border_b(theme::border())
                .border_color(t.line_soft)
                .flex()
                .items_center()
                .gap(SPACE_2)
                .child(vault_ui::pill(
                    vault_ui::protection_rule(VaultCategory::Payments),
                    t.danger,
                    Some(Icon::Fingerprint),
                    cx,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(theme::ui_text(TEXT_2XS))
                        .text_color(t.text_tertiary)
                        .child(t!("Always approval-gated with Touch ID. Card numbers and CVCs are never exposed to agents.")),
                )
                .into_any_element(),
        ),
        Tab::Docs => Some(
            div()
                .px(SPACE_4)
                .py(SPACE_2)
                .border_b(theme::border())
                .border_color(t.line_soft)
                .flex()
                .items_center()
                .gap(SPACE_2)
                .child(vault_ui::pill(
                    vault_ui::protection_rule(VaultCategory::Documents),
                    t.warn,
                    Some(Icon::ShieldCheck),
                    cx,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(theme::ui_text(TEXT_2XS))
                        .text_color(t.text_tertiary)
                        .child(t!("Encrypted locally and staged into private 0600 temp files for blind browser uploads.")),
                )
                .into_any_element(),
        ),
        Tab::Activity => None,
    };

    let content = match fields.tab {
        Tab::Activity => render_activity(fields, cx),
        tab => {
            let category = tab.category().unwrap_or(VaultCategory::Details);
            render_entries(fields, category, this, cx)
        }
    };

    let meta = if fields.loading {
        Some(t!("Loading…").into())
    } else {
        Some(t!("AES-256-GCM · zero-exposure host fill").into())
    };

    let body = div()
        .flex()
        .flex_col()
        .child(controls)
        .children(tab_banner)
        .child(content);

    vec![rows::section(
        n!("Vault"),
        meta,
        None,
        lit("Vault"),
        body,
        cx,
    )]
}

fn render_entries(
    fields: &Fields,
    category: VaultCategory,
    this: &Entity<Workbench>,
    cx: &App,
) -> AnyElement {
    let query = fields.search.read(cx).value().trim().to_lowercase();
    let shown: Vec<&VaultEntrySummary> = fields
        .entries
        .iter()
        .filter(|e| e.scope.key() == fields.scope.key() && e.category == category)
        .filter(|e| {
            if query.is_empty() {
                return true;
            }
            e.title.to_lowercase().contains(&query)
                || e.domains.iter().any(|d| d.to_lowercase().contains(&query))
                || e.fields.iter().any(|f| {
                    f.label.to_lowercase().contains(&query) || f.key.to_lowercase().contains(&query)
                })
        })
        .collect();

    if shown.is_empty() {
        let (title, body) = match category {
            VaultCategory::Details => (
                t!("No details saved yet"),
                t!("Save your personal or business profile once — or let an agent prompt you inline the first time it hits a form."),
            ),
            VaultCategory::Secrets => (
                t!("No secrets saved yet"),
                t!("Store API keys, logins, and 2FA seeds encrypted locally, or link 1Password op:// references for host blind-fill."),
            ),
            VaultCategory::Payments => (
                t!("No payment cards saved yet"),
                t!("Add a card for biometric-gated blind checkout. Card numbers and CVCs are never shown to agents."),
            ),
            VaultCategory::Documents => (
                t!("No encrypted documents yet"),
                t!("Attach PAN, Aadhaar, passport, or agreements to upload blind into browser forms without exposing the file to agents."),
            ),
        };
        return vault_ui::empty_state(vault_ui::category_icon(category), title, body, cx)
            .into_any_element();
    }

    div()
        .flex()
        .flex_col()
        .children(shown.into_iter().map(|entry| {
            let expanded = fields.selected.as_deref() == Some(entry.id.as_str());
            render_entry_row(entry, expanded, fields, this, cx)
        }))
        .into_any_element()
}

fn render_entry_row(
    entry: &VaultEntrySummary,
    expanded: bool,
    fields: &Fields,
    this: &Entity<Workbench>,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let id = entry.id.clone();
    let title = entry.title.clone();
    let subtitle = entry
        .subtitle
        .clone()
        .unwrap_or_else(|| format!("{} fields", entry.fields.len()));

    let brand_badge = (entry.category == VaultCategory::Payments).then(|| {
        let hint = entry
            .fields
            .iter()
            .find(|f| f.kind == VaultFieldKind::CardNumber)
            .and_then(|f| f.masked_hint.as_deref())
            .unwrap_or(&subtitle);
        vault_ui::card_brand_mark(CardBrand::detect(hint, &entry.title), cx)
    });

    let source_badge = matches!(entry.source, VaultSource::OnePassword { .. })
        .then(|| vault_ui::onepassword_badge(None, cx));

    let domain_badge = entry
        .domains
        .first()
        .map(|domain| vault_ui::domain_chip(domain, cx));

    let select_this = this.clone();
    let select_id = id.clone();
    let edit_this = this.clone();
    let edit_id = id.clone();
    let del_this = this.clone();
    let del_id = id.clone();
    let del_title = title.clone();

    let header = div()
        .id(SharedString::from(format!("vault-row-{id}")))
        .px(SPACE_4)
        .py(px(10.))
        .flex()
        .items_center()
        .gap(SPACE_3)
        .cursor_pointer()
        .when(expanded, |row| row.bg(t.surface_chosen))
        .when(!expanded, |row| row.hover(move |s| s.bg(t.surface_hover)))
        .clicks(move |_, _, cx| {
            select_this.update(cx, |bench, cx| {
                change_fields(bench, cx, |fields, _| {
                    if fields.selected.as_deref() == Some(select_id.as_str()) {
                        fields.selected = None;
                    } else {
                        fields.selected = Some(select_id.clone());
                    }
                    fields.revealed = None;
                });
            });
        })
        .child(vault_ui::icon_tile(entry.category, &entry.source, 28., cx))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(SPACE_0)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(SPACE_2)
                        .child(
                            div()
                                .truncate()
                                .text_size(theme::ui_text(TEXT_SM))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(t.text_primary)
                                .child(entry.title.clone()),
                        )
                        .children(brand_badge)
                        .children(source_badge),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(theme::ui_text(TEXT_2XS))
                        .text_color(t.text_tertiary)
                        .child(subtitle),
                ),
        )
        .children(domain_badge)
        .child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(SPACE_1)
                .when(entry.category == VaultCategory::Documents, |acts| {
                    let open_this = this.clone();
                    let open_id = id.clone();
                    acts.child(
                        ui::Button::new(
                            SharedString::from(format!("vault-open-doc-{id}")),
                            t!("Open"),
                            ui::Variant::Outline,
                        )
                        .sized(24., 8., 11.)
                        .on_click(move |_, _, cx| {
                            cx.stop_propagation();
                            open_this.update(cx, |bench, cx| {
                                open_document(bench, open_id.clone(), false, cx)
                            });
                        }),
                    )
                })
                .child(
                    ui::IconButton::new(
                        SharedString::from(format!("vault-edit-{id}")),
                        Icon::Pencil,
                        t!("Edit"),
                    )
                    .compact()
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        edit_this.update(cx, |bench, cx| {
                            open_edit_editor(bench, edit_id.clone(), window, cx)
                        });
                    }),
                )
                .child(
                    ui::IconButton::new(
                        SharedString::from(format!("vault-delete-{id}")),
                        Icon::Trash,
                        t!("Delete"),
                    )
                    .compact()
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        del_this.update(cx, |bench, cx| {
                            change_fields(bench, cx, |fields, _| {
                                fields.confirm_delete = Some((del_id.clone(), del_title.clone()));
                            });
                        });
                    }),
                ),
        );

    let inspector = expanded.then(|| render_entry_detail(entry, fields, this, cx));

    div()
        .border_b(theme::border())
        .border_color(t.line_soft)
        .flex()
        .flex_col()
        .child(header)
        .children(inspector)
        .into_any_element()
}

fn render_entry_detail(
    entry: &VaultEntrySummary,
    fields: &Fields,
    this: &Entity<Workbench>,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let revealed = fields
        .revealed
        .as_ref()
        .filter(|d| d.summary.id == entry.id && d.revealed);
    let has_sensitive = entry
        .fields
        .iter()
        .any(|f| f.sensitive || f.kind.is_inherently_sensitive());

    let card_preview = (entry.category == VaultCategory::Payments).then(|| {
        let num_hint = entry
            .fields
            .iter()
            .find(|f| f.kind == VaultFieldKind::CardNumber)
            .and_then(|f| f.masked_hint.as_deref())
            .unwrap_or("•••• •••• •••• ••••");
        let holder = revealed
            .and_then(|d| {
                d.fields
                    .iter()
                    .find(|f| f.key == "cardholder")
                    .and_then(|f| f.value.as_deref())
            })
            .or_else(|| {
                entry
                    .fields
                    .iter()
                    .find(|f| f.key == "cardholder")
                    .and_then(|f| f.masked_hint.as_deref())
            })
            .unwrap_or("CARDHOLDER");
        let exp = revealed
            .and_then(|d| {
                d.fields
                    .iter()
                    .find(|f| f.kind == VaultFieldKind::Expiry)
                    .and_then(|f| f.value.as_deref())
            })
            .or_else(|| {
                entry
                    .fields
                    .iter()
                    .find(|f| f.kind == VaultFieldKind::Expiry)
                    .and_then(|f| f.masked_hint.as_deref())
            })
            .unwrap_or("••/••");
        div().max_w(px(340.)).child(vault_ui::dark_glass_card(
            CardBrand::detect(num_hint, &entry.title),
            &vault_ui::mask_card_last4(num_hint),
            holder,
            exp,
            Some(&entry.title),
            cx,
        ))
    });

    let doc_box = entry.document.as_ref().map(|doc| {
        let reveal_this = this.clone();
        let open_this = this.clone();
        let id = entry.id.clone();
        let open_id = entry.id.clone();
        div()
            .px(SPACE_3)
            .py(SPACE_2)
            .rounded(theme::radius_2())
            .bg(t.surface_sunken)
            .border(theme::border())
            .border_color(t.border)
            .flex()
            .items_center()
            .gap(SPACE_3)
            .child(ui::icon(Icon::File, 16.).text_color(t.warn))
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
                            .child(doc.filename.clone()),
                    )
                    .child(
                        div()
                            .text_size(theme::ui_text(TEXT_3XS))
                            .font_family(theme::mono_font())
                            .text_color(t.text_tertiary)
                            .child(format!(
                                "{} · {} · sha256:{}",
                                vault_ui::format_bytes(doc.byte_len),
                                doc.mime_type,
                                doc.sha256_prefix
                            )),
                    ),
            )
            .child(
                ui::Button::new(
                    SharedString::from(format!("vault-doc-reveal-{id}")),
                    t!("Reveal in Finder"),
                    ui::Variant::Ghost,
                )
                .sized(24., 8., 11.)
                .on_click(move |_, _, cx| {
                    reveal_this.update(cx, |bench, cx| open_document(bench, id.clone(), true, cx))
                }),
            )
            .child(
                ui::Button::new(
                    SharedString::from(format!("vault-doc-open-{open_id}")),
                    t!("Open"),
                    ui::Variant::Outline,
                )
                .sized(24., 8., 11.)
                .on_click(move |_, _, cx| {
                    open_this.update(cx, |bench, cx| {
                        open_document(bench, open_id.clone(), false, cx)
                    })
                }),
            )
    });

    let field_rows: Vec<AnyElement> = entry
        .fields
        .iter()
        .map(|f| {
            let revealed_val = revealed
                .and_then(|d| d.fields.iter().find(|rf| rf.key == f.key))
                .and_then(|rf| rf.value.clone());
            let display_val = if let Some(val) = &revealed_val {
                val.clone()
            } else if f.kind == VaultFieldKind::Totp {
                t!("Generated on host at fill (6 digits)").to_owned()
            } else if let Some(op) = &f.op_ref {
                op.clone()
            } else if let Some(hint) = &f.masked_hint {
                hint.clone()
            } else {
                "••••••••".to_owned()
            };
            let is_mono = f.sensitive
                || f.kind.is_inherently_sensitive()
                || f.op_ref.is_some()
                || matches!(f.kind, VaultFieldKind::Expiry | VaultFieldKind::Phone);
            let copy_val = revealed_val.filter(|_| entry.category != VaultCategory::Payments);
            let copy_this = this.clone();
            let copy_key = format!("vault-copy-{}-{}", entry.id, f.key);
            div()
                .py(px(6.))
                .border_b(theme::border())
                .border_color(t.line_soft)
                .flex()
                .items_center()
                .gap(SPACE_3)
                .child(
                    div()
                        .w(px(132.))
                        .flex_none()
                        .truncate()
                        .text_size(theme::ui_text(TEXT_XS))
                        .text_color(t.text_tertiary)
                        .child(f.label.clone()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(theme::ui_text(TEXT_XS))
                        .when(is_mono, |v| v.font_family(theme::mono_font()))
                        .text_color(t.text_primary)
                        .child(display_val),
                )
                .children(copy_val.map(|val| {
                    ui::IconButton::new(SharedString::from(copy_key), Icon::Copy, t!("Copy"))
                        .small()
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(val.clone()));
                            copy_this.update(cx, |bench, cx| {
                                bench.notify(toast::success(t!("Copied to clipboard")), cx)
                            });
                        })
                }))
                .into_any_element()
        })
        .collect();

    let reveal_btn = has_sensitive.then(|| {
        let reveal_this = this.clone();
        let id = entry.id.clone();
        let is_open = revealed.is_some();
        ui::Button::new(
            SharedString::from(format!("vault-reveal-{id}")),
            if is_open { t!("Hide") } else { t!("Reveal") },
            ui::Variant::Outline,
        )
        .icon(if is_open { Icon::EyeOff } else { Icon::Eye })
        .sized(24., 9., 11.)
        .disabled(fields.revealing)
        .on_click(move |_, _, cx| {
            reveal_this.update(cx, |bench, cx| toggle_reveal(bench, id.clone(), cx))
        })
    });

    let domains_row =
        (!entry.domains.is_empty() || !entry.approved_domains.is_empty()).then(|| {
            let mut all_domains: Vec<&str> = entry.domains.iter().map(String::as_str).collect();
            for d in &entry.approved_domains {
                if !all_domains.contains(&d.as_str()) {
                    all_domains.push(d.as_str());
                }
            }
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(SPACE_1H)
                .child(
                    div()
                        .text_size(theme::ui_text(TEXT_3XS))
                        .text_color(t.text_tertiary)
                        .child(t!("ALLOWED ON")),
                )
                .children(
                    all_domains
                        .into_iter()
                        .map(|d| vault_ui::domain_chip(d, cx)),
                )
        });

    let last_used = entry.last_used_at.map(|ts| {
        div()
            .text_size(theme::ui_text(TEXT_3XS))
            .text_color(t.text_tertiary)
            .child(t!("Last blind-filled {when}", when = ago(ts)))
    });

    div()
        .px(SPACE_4)
        .pt(SPACE_2)
        .pb(SPACE_3)
        .bg(t.surface_raised.opacity(0.6))
        .flex()
        .flex_col()
        .gap(SPACE_2)
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(SPACE_2)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(SPACE_1H)
                        .child(vault_ui::pill(
                            entry.delivery_note.clone(),
                            vault_ui::category_color(entry.category, &t),
                            Some(Icon::Shield),
                            cx,
                        ))
                        .children(last_used),
                )
                .children(reveal_btn),
        )
        .children(card_preview)
        .children(doc_box)
        .when(!field_rows.is_empty(), |col| {
            col.child(div().flex().flex_col().children(field_rows))
        })
        .children(domains_row)
        .into_any_element()
}

fn render_activity(fields: &Fields, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    if fields.audit.is_empty() {
        return vault_ui::empty_state(
            Icon::ShieldCheck,
            t!("No Vault activity yet"),
            t!("Every zero-exposure blind-fill, blind-upload, approval, and save is logged here with metadata only — never secret values."),
            cx,
        )
        .into_any_element();
    }
    div()
        .flex()
        .flex_col()
        .children(fields.audit.iter().map(|row| {
            let (status_word, status_color, status_icon) =
                vault_ui::audit_status_meta(row.status, t);
            let actor = vault_ui::format_actor(&row.actor);
            let fields_str = if row.field_keys.is_empty() {
                String::new()
            } else {
                format!(" · {}", row.field_keys.join(", "))
            };
            div()
                .px(SPACE_4)
                .py(px(9.))
                .border_b(theme::border())
                .border_color(t.line_soft)
                .flex()
                .items_center()
                .gap(SPACE_3)
                .child(
                    div()
                        .size(px(24.))
                        .flex_none()
                        .rounded(theme::radius_2())
                        .bg(status_color.opacity(0.12))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(ui::icon(status_icon, 12.).text_color(status_color)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(SPACE_0)
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(SPACE_1H)
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(theme::ui_text(TEXT_XS))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(t.text_primary)
                                        .child(row.entry_title.clone()),
                                )
                                .child(vault_ui::pill(status_word, status_color, None, cx))
                                .children(
                                    row.domain.as_deref().map(|d| vault_ui::domain_chip(d, cx)),
                                ),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(theme::ui_text(TEXT_2XS))
                                .text_color(t.text_tertiary)
                                .child(format!("{actor}{fields_str}")),
                        ),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(theme::ui_text(TEXT_2XS))
                        .text_color(t.text_tertiary)
                        .child(ago(row.at)),
                )
        }))
        .into_any_element()
}

pub(super) fn dialog(
    bench: &Workbench,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> Option<AnyElement> {
    let fields = super::state(bench, cx)?.vault.as_ref()?;
    if let Some((id, title)) = &fields.confirm_delete {
        let del_this = this.clone();
        let cancel_this = this.clone();
        let id = id.clone();
        let title_clone = title.clone();
        return Some(
            ui::Confirm::new(
                t!("Remove “{title}” from Vault?", title = title),
                t!("Its encrypted data will be permanently deleted from this host. Agents will no longer be able to blind-fill it."),
                t!("Delete"),
            )
            .destructive(true)
            .marked()
            .on_yes(move |_, cx| {
                del_this.update(cx, |bench, cx| {
                    delete_entry(bench, id.clone(), title_clone.clone(), cx)
                });
            })
            .on_no(move |_, cx| {
                cancel_this.update(cx, |bench, cx| {
                    change_fields(bench, cx, |fields, _| fields.confirm_delete = None);
                });
            })
            .into_any_element(),
        );
    }

    let editor = fields.editor.as_ref()?;
    let t = Tokens::get(cx).clone();
    let heading: SharedString = match (editor.id.is_some(), editor.category) {
        (true, VaultCategory::Details) => t!("Edit detail").into(),
        (false, VaultCategory::Details) => t!("Add detail").into(),
        (true, VaultCategory::Secrets) => t!("Edit secret").into(),
        (false, VaultCategory::Secrets) => t!("Add secret").into(),
        (true, VaultCategory::Payments) => t!("Edit payment card").into(),
        (false, VaultCategory::Payments) => t!("Add payment card").into(),
        (true, VaultCategory::Documents) => t!("Edit document").into(),
        (false, VaultCategory::Documents) => t!("Add document").into(),
    };

    let source_toggle = (editor.category == VaultCategory::Secrets).then(|| {
        let this = this.clone();
        let selected = usize::from(editor.source_onepassword);
        let source_now = if editor.source_onepassword {
            VaultSource::OnePassword {
                reference: String::new(),
            }
        } else {
            VaultSource::Sikemux
        };
        div()
            .flex()
            .flex_col()
            .gap(SPACE_2)
            .child(
                ui::Segmented::words(
                    "vault-editor-source",
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
                    this.update(cx, |bench, cx| {
                        change_fields(bench, cx, |fields, _| {
                            if let Some(editor) = fields.editor.as_mut() {
                                editor.source_onepassword = is_op;
                                editor.error = None;
                            }
                        });
                    });
                }),
            )
            .child(vault_ui::honest_privacy_badge(&source_now, cx))
    });

    let live_card = (editor.category == VaultCategory::Payments).then(|| {
        let num = editor
            .fields
            .iter()
            .find(|f| f.kind == VaultFieldKind::CardNumber)
            .map(|f| f.value_input.read(cx).value().to_string())
            .unwrap_or_default();
        let holder = editor
            .fields
            .iter()
            .find(|f| f.key == "cardholder")
            .map(|f| f.value_input.read(cx).value().to_string())
            .unwrap_or_default();
        let exp = editor
            .fields
            .iter()
            .find(|f| f.kind == VaultFieldKind::Expiry)
            .map(|f| f.value_input.read(cx).value().to_string())
            .unwrap_or_default();
        let title_txt = editor.title.read(cx).value().to_string();
        vault_ui::dark_glass_card(
            CardBrand::detect(&num, &title_txt),
            &vault_ui::mask_card_last4(&num),
            &holder,
            &exp,
            (!title_txt.trim().is_empty()).then_some(title_txt.trim()),
            cx,
        )
    });

    let doc_picker = (editor.category == VaultCategory::Documents).then(|| {
        let pick_this = this.clone();
        let (name, sub, ready) = if let Some(path) = &editor.doc_path {
            let fname = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.to_string_lossy().into_owned());
            let sz = std::fs::metadata(path)
                .map(|m| vault_ui::format_bytes(m.len()))
                .unwrap_or_else(|_| t!("Ready").to_owned());
            (fname, format!("{sz} · encrypted on save"), true)
        } else if let Some(existing) = &editor.doc_existing {
            (
                existing.filename.clone(),
                format!(
                    "{} · {} · click to replace",
                    vault_ui::format_bytes(existing.byte_len),
                    existing.mime_type
                ),
                true,
            )
        } else {
            (
                t!("Choose a file to encrypt in Vault").to_owned(),
                t!("PDF, image, or ID document · up to 25 MB").to_owned(),
                false,
            )
        };
        div()
            .id("vault-editor-pick-doc")
            .w_full()
            .px(SPACE_3)
            .py(SPACE_3)
            .rounded(theme::radius_2())
            .border(theme::border())
            .when(!ready, |box_| {
                box_.border_dashed().border_color(t.border_strong)
            })
            .when(ready, |box_| box_.border_color(t.live.opacity(0.45)))
            .bg(t.surface_sunken)
            .hover(move |s| s.bg(t.surface_hover))
            .cursor_pointer()
            .flex()
            .items_center()
            .gap(SPACE_3)
            .clicks(move |_, window, cx| {
                pick_this.update(cx, |bench, cx| pick_editor_document(bench, window, cx))
            })
            .child(
                div()
                    .size(px(32.))
                    .flex_none()
                    .rounded(theme::radius_2())
                    .bg(t.surface_raised)
                    .border(theme::border())
                    .border_color(t.border)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(ui::icon(Icon::File, 15.).text_color(if ready {
                        t.live
                    } else {
                        t.text_secondary
                    })),
            )
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
            .when(ready, |box_| {
                box_.child(vault_ui::pill(t!("Ready"), t.live, Some(Icon::Check), cx))
            })
    });

    let op_field =
        (editor.category == VaultCategory::Secrets && editor.source_onepassword).then(|| {
            div()
                .flex()
                .flex_col()
                .gap(SPACE_1)
                .child(vault_ui::field_label(
                    t!("1Password reference"),
                    Some(t!("Resolved on demand via op CLI")),
                    cx,
                ))
                .child(ui::Field::new(&editor.op_ref).mono().small())
        });

    let field_inputs: Vec<AnyElement> =
        if editor.category == VaultCategory::Secrets && editor.source_onepassword {
            Vec::new()
        } else {
            editor
                .fields
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
                    let toggle_eye = f.sensitive.then(|| {
                        let eye_this = this.clone();
                        let masked = f.masked;
                        ui::IconButton::new(
                            SharedString::from(format!("vault-editor-eye-{ix}")),
                            if masked { Icon::Eye } else { Icon::EyeOff },
                            if masked { t!("Show") } else { t!("Hide") },
                        )
                        .compact()
                        .on_click(move |_, window, cx| {
                            eye_this.update(cx, |bench, cx| {
                                change_fields(bench, cx, |fields, cx| {
                                    if let Some(field) =
                                        fields.editor.as_mut().and_then(|e| e.fields.get_mut(ix))
                                    {
                                        field.masked = !field.masked;
                                        let next = field.masked;
                                        field
                                            .value_input
                                            .update(cx, |i, cx| i.set_masked(next, window, cx));
                                    }
                                });
                            });
                        })
                    });
                    let remove_btn = f.custom.then(|| {
                        let rm_this = this.clone();
                        ui::IconButton::new(
                            SharedString::from(format!("vault-editor-rm-{ix}")),
                            Icon::Close,
                            t!("Remove field"),
                        )
                        .compact()
                        .on_click(move |_, _, cx| {
                            rm_this.update(cx, |bench, cx| {
                                change_fields(bench, cx, |fields, _| {
                                    if let Some(editor) = fields.editor.as_mut() {
                                        if ix < editor.fields.len() {
                                            editor.fields.remove(ix);
                                        }
                                    }
                                });
                            });
                        })
                    });
                    let value_field = {
                        let base = ui::Field::new(&f.value_input).small();
                        if is_mono {
                            base.mono()
                        } else {
                            base
                        }
                    };
                    if f.custom {
                        div()
                            .flex()
                            .items_center()
                            .gap(SPACE_1H)
                            .child(
                                div()
                                    .w(px(140.))
                                    .child(ui::Field::new(&f.label_input).small()),
                            )
                            .child(div().flex_1().min_w_0().child(value_field))
                            .children(toggle_eye)
                            .children(remove_btn)
                            .into_any_element()
                    } else {
                        let label_txt = f.label_input.read(cx).value().to_string();
                        let hint = match f.kind {
                            VaultFieldKind::Totp => Some(t!("6-digit codes generated on host")),
                            VaultFieldKind::Cvc => Some(t!("Encrypted · never exposed to agents")),
                            _ => None,
                        };
                        div()
                            .flex()
                            .flex_col()
                            .gap(SPACE_1)
                            .child(vault_ui::field_label(label_txt, hint, cx))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(SPACE_1H)
                                    .child(div().flex_1().min_w_0().child(value_field))
                                    .children(toggle_eye),
                            )
                            .into_any_element()
                    }
                })
                .collect()
        };

    let add_field_btn = matches!(
        editor.category,
        VaultCategory::Details | VaultCategory::Secrets
    ) && !(editor.category == VaultCategory::Secrets
        && editor.source_onepassword);
    let add_field_row = add_field_btn.then(|| {
        let add_this = this.clone();
        div().flex().child(
            ui::Button::new(
                "vault-editor-add-field",
                t!("Add custom field"),
                ui::Variant::Dashed,
            )
            .icon(Icon::Plus)
            .on_click(move |_, window, cx| {
                add_this.update(cx, |bench, cx| add_custom_editor_field(bench, window, cx))
            }),
        )
    });

    let approval_row = (editor.category != VaultCategory::Payments).then(|| {
        let sw_this = this.clone();
        div()
            .pt(SPACE_1)
            .flex()
            .items_center()
            .justify_between()
            .gap(SPACE_2)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(SPACE_0)
                    .child(
                        div()
                            .text_size(theme::ui_text(TEXT_XS))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(t.text_primary)
                            .child(t!("Ask before blind-filling")),
                    )
                    .child(
                        div()
                            .text_size(theme::ui_text(TEXT_3XS))
                            .text_color(t.text_tertiary)
                            .child(t!(
                                "Require explicit approval each time an agent fills this item."
                            )),
                    ),
            )
            .child(
                Switch::new("vault-editor-approval")
                    .checked(editor.require_approval)
                    .on_change(move |on: &bool, _, cx| {
                        let on = *on;
                        sw_this.update(cx, |bench, cx| {
                            change_fields(bench, cx, |fields, _| {
                                if let Some(editor) = fields.editor.as_mut() {
                                    editor.require_approval = on;
                                }
                            });
                        });
                    }),
            )
    });

    let body = div()
        .id("vault-editor-scroll")
        .max_h(px(520.))
        .overflow_y_scroll()
        .px(SPACE_5)
        .py(SPACE_3)
        .flex()
        .flex_col()
        .gap(SPACE_3)
        .children(source_toggle)
        .children(live_card)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(SPACE_1)
                .child(vault_ui::field_label(t!("Label"), None, cx))
                .child(ui::Field::new(&editor.title).small()),
        )
        .children(doc_picker)
        .children(op_field)
        .children(field_inputs)
        .children(add_field_row)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(SPACE_1)
                .child(vault_ui::field_label(
                    t!("Bound domains"),
                    Some(t!("Optional · comma-separated")),
                    cx,
                ))
                .child(ui::Field::new(&editor.domains).mono().small()),
        )
        .children(approval_row)
        .children(editor.error.clone().map(|err| {
            div()
                .text_size(theme::ui_text(TEXT_XS))
                .text_color(t.danger)
                .child(err)
        }))
        .into_any_element();

    let save_this = this.clone();
    let yes: dialog::Answer = Rc::new(move |_, cx| {
        save_this.update(cx, save_editor);
    });
    let cancel_this = this.clone();
    let no: dialog::Answer = Rc::new(move |_, cx| {
        cancel_this.update(cx, |bench, cx| {
            change_fields(bench, cx, |fields, _| fields.editor = None);
        });
    });

    let foot = dialog::hinted_foot(dialog::escape(t!("cancel"), cx), cx)
        .child(dialog::button(
            "vault-editor-cancel",
            t!("Cancel").into(),
            None,
            no.clone(),
            cx,
        ))
        .child(
            dialog::button(
                "vault-editor-save",
                t!("Save to Vault").into(),
                Some(t.accent),
                yes.clone(),
                cx,
            )
            .busy(editor.saving),
        );

    Some(
        dialog::sheet_of(px(520.), heading, body, foot, yes, no, cx)
            .keys_to(Some(editor.title.read(cx).focus_handle(cx)))
            .into_any_element(),
    )
}
