//! Shared Agentic Vault UI primitives: category styling, 1Password and payment brand marks,
//! tactile dark-glass payment card preview, honest privacy badge, and safe display formatters.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, linear_color_stop, linear_gradient, px, AnyElement, App, Div, FontWeight, Hsla,
    IntoElement, ParentElement, SharedString, Styled,
};
use sikemux_calls::vault::{VaultAuditStatus, VaultCategory, VaultSource};

use crate::icons::Icon;
use crate::theme::{
    self, Tokens, SPACE_0, SPACE_1H, SPACE_2, SPACE_3, SPACE_4, TEXT_2XS, TEXT_3XS, TEXT_SM,
    TEXT_XS,
};
use crate::ui;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardBrand {
    Visa,
    Mastercard,
    Amex,
    RuPay,
    Generic,
}

impl CardBrand {
    pub fn detect(raw: &str, title: &str) -> Self {
        let digits: String = raw.chars().filter(char::is_ascii_digit).collect();
        if digits.starts_with('4') {
            return Self::Visa;
        }
        if digits.starts_with("34") || digits.starts_with("37") {
            return Self::Amex;
        }
        if let Some(Ok(prefix2)) = digits.get(..2).map(str::parse::<u32>) {
            if (51..=55).contains(&prefix2) {
                return Self::Mastercard;
            }
        }
        if let Some(Ok(prefix4)) = digits.get(..4).map(str::parse::<u32>) {
            if (2221..=2720).contains(&prefix4) {
                return Self::Mastercard;
            }
        }
        if digits.starts_with("60")
            || digits.starts_with("65")
            || digits.starts_with("81")
            || digits.starts_with("82")
        {
            return Self::RuPay;
        }
        let lower = title.to_ascii_lowercase();
        if lower.contains("visa") {
            Self::Visa
        } else if lower.contains("mastercard") || lower.contains("master card") {
            Self::Mastercard
        } else if lower.contains("amex") || lower.contains("american express") {
            Self::Amex
        } else if lower.contains("rupay") {
            Self::RuPay
        } else {
            Self::Generic
        }
    }
}

pub fn category_icon(category: VaultCategory) -> Icon {
    match category {
        VaultCategory::Details => Icon::User,
        VaultCategory::Secrets => Icon::Key,
        VaultCategory::Payments => Icon::CreditCard,
        VaultCategory::Documents => Icon::File,
    }
}

pub fn category_color(category: VaultCategory, t: &Tokens) -> Hsla {
    match category {
        VaultCategory::Details => t.live,
        VaultCategory::Secrets => t.warn,
        VaultCategory::Payments => t.danger,
        VaultCategory::Documents => t.warn,
    }
}

pub fn category_label(category: VaultCategory) -> &'static str {
    match category {
        VaultCategory::Details => t!("Details"),
        VaultCategory::Secrets => t!("Secrets"),
        VaultCategory::Payments => t!("Payments"),
        VaultCategory::Documents => t!("Docs"),
    }
}

pub fn protection_rule(category: VaultCategory) -> &'static str {
    match category {
        VaultCategory::Details => t!("Fills on trusted sites"),
        VaultCategory::Secrets => t!("Ask before filling"),
        VaultCategory::Payments => t!("Touch ID every time"),
        VaultCategory::Documents => t!("Encrypted · blind upload"),
    }
}

pub fn audit_status_meta(status: VaultAuditStatus, t: &Tokens) -> (&'static str, Hsla, Icon) {
    match status {
        VaultAuditStatus::Filled => (t!("Blind-filled"), t.live, Icon::ShieldCheck),
        VaultAuditStatus::Uploaded => (t!("Blind-uploaded"), t.live, Icon::Upload),
        VaultAuditStatus::Approved => (t!("Approved"), t.live, Icon::Check),
        VaultAuditStatus::Denied => (t!("Denied"), t.danger, Icon::Close),
        VaultAuditStatus::Failed => (t!("Failed"), t.danger, Icon::Warning),
        VaultAuditStatus::Created => (t!("Saved"), t.accent, Icon::Plus),
        VaultAuditStatus::Updated => (t!("Updated"), t.accent, Icon::Pencil),
        VaultAuditStatus::Deleted => (t!("Deleted"), t.text_tertiary, Icon::Trash),
        VaultAuditStatus::Revealed => (t!("Revealed"), t.warn, Icon::Eye),
    }
}

pub fn pill(label: impl Into<SharedString>, color: Hsla, glyph: Option<Icon>, cx: &App) -> Div {
    let _ = cx;
    div()
        .flex_none()
        .h(px(20.))
        .px(px(7.))
        .flex()
        .items_center()
        .gap(px(5.))
        .rounded(theme::radius_1())
        .bg(color.opacity(0.12))
        .text_size(theme::ui_text(10.5))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(color)
        .whitespace_nowrap()
        .children(glyph.map(|g| ui::icon(g, 11.).text_color(color)))
        .child(label.into())
}

pub fn domain_chip(domain: &str, cx: &App) -> Div {
    let t = Tokens::get(cx);
    div()
        .flex_none()
        .h(px(20.))
        .px(px(7.))
        .flex()
        .items_center()
        .gap(px(5.))
        .rounded(theme::radius_1())
        .bg(t.surface)
        .border(theme::border())
        .border_color(t.border)
        .font_family(theme::mono_font())
        .text_size(theme::ui_text(10.5))
        .text_color(t.text_secondary)
        .whitespace_nowrap()
        .child(ui::icon(Icon::Lock, 10.).text_color(t.live))
        .child(domain.to_owned())
}

pub fn onepassword_badge(reference: Option<&str>, cx: &App) -> Div {
    let t = Tokens::get(cx);
    let text: SharedString = match reference {
        Some(r) if !r.is_empty() => r.to_owned().into(),
        _ => "1Password".into(),
    };
    div()
        .flex_none()
        .h(px(20.))
        .px(px(7.))
        .flex()
        .items_center()
        .gap(px(5.))
        .rounded(theme::radius_1())
        .bg(t.surface_chosen)
        .border(theme::border())
        .border_color(t.border)
        .text_size(theme::ui_text(10.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(t.text_primary)
        .whitespace_nowrap()
        .child(ui::picture(Icon::OnePassword, 13., t.accent))
        .child(
            div()
                .max_w(px(220.))
                .truncate()
                .when(reference.is_some(), |d| d.font_family(theme::mono_font()))
                .child(text),
        )
}

pub fn card_brand_mark(brand: CardBrand, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    match brand {
        CardBrand::Visa => div()
            .flex_none()
            .px(px(6.))
            .py(px(2.))
            .rounded(theme::radius_1())
            .bg(gpui_kit::hsla(222. / 360., 0.85, 0.56, 0.2))
            .text_size(theme::ui_text(11.))
            .font_weight(FontWeight::BOLD)
            .italic()
            .letter_spacing(0.08)
            .text_color(gpui_kit::hsla(220. / 360., 0.9, 0.78, 1.))
            .child("VISA")
            .into_any_element(),
        CardBrand::Mastercard => div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(5.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .size(px(11.))
                            .rounded_full()
                            .bg(gpui_kit::hsla(0.01, 0.85, 0.54, 0.92)),
                    )
                    .child(
                        div()
                            .ml(px(-4.))
                            .size(px(11.))
                            .rounded_full()
                            .bg(gpui_kit::hsla(0.11, 0.92, 0.55, 0.88)),
                    ),
            )
            .child(
                div()
                    .text_size(theme::ui_text(10.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.text_secondary)
                    .child("Mastercard"),
            )
            .into_any_element(),
        CardBrand::Amex => div()
            .flex_none()
            .px(px(6.))
            .py(px(2.))
            .rounded(theme::radius_1())
            .bg(gpui_kit::hsla(204. / 360., 0.85, 0.52, 0.22))
            .text_size(theme::ui_text(10.5))
            .font_weight(FontWeight::BOLD)
            .letter_spacing(0.06)
            .text_color(gpui_kit::hsla(204. / 360., 0.9, 0.76, 1.))
            .child("AMEX")
            .into_any_element(),
        CardBrand::RuPay => div()
            .flex_none()
            .px(px(6.))
            .py(px(2.))
            .rounded(theme::radius_1())
            .bg(gpui_kit::hsla(28. / 360., 0.88, 0.54, 0.2))
            .text_size(theme::ui_text(10.5))
            .font_weight(FontWeight::BOLD)
            .text_color(gpui_kit::hsla(28. / 360., 0.92, 0.74, 1.))
            .child("RuPay")
            .into_any_element(),
        CardBrand::Generic => ui::icon(Icon::CreditCard, 15.)
            .text_color(t.text_secondary)
            .into_any_element(),
    }
}

pub fn mask_card_last4(raw: &str) -> String {
    let digits: String = raw.chars().filter(char::is_ascii_digit).collect();
    if let Some(last4) = digits
        .len()
        .checked_sub(4)
        .and_then(|start| digits.get(start..))
    {
        format!("•••• •••• •••• {last4}")
    } else if raw.contains('•') && !raw.trim().is_empty() {
        raw.trim().to_owned()
    } else {
        "•••• •••• •••• ••••".to_owned()
    }
}

pub fn dark_glass_card(
    brand: CardBrand,
    masked_number: &str,
    cardholder: &str,
    expiry: &str,
    label: Option<&str>,
    cx: &App,
) -> Div {
    let t = Tokens::get(cx);
    let top_bg = gpui_kit::hsla(250. / 360., 0.16, 0.14, 1.);
    let bottom_bg = gpui_kit::hsla(245. / 360., 0.14, 0.08, 1.);
    let chip_bg = gpui_kit::hsla(240. / 360., 0.10, 0.24, 1.);
    let edge = gpui_kit::hsla(0., 0., 1., 0.12);
    let ink = gpui_kit::hsla(252. / 360., 0.24, 0.92, 1.);
    let muted = gpui_kit::hsla(252. / 360., 0.12, 0.64, 1.);
    let holder = if cardholder.trim().is_empty() {
        "CARDHOLDER".to_owned()
    } else {
        cardholder.trim().to_uppercase()
    };
    let exp = if expiry.trim().is_empty() {
        "MM/YY".to_owned()
    } else {
        expiry.trim().to_owned()
    };
    div()
        .w_full()
        .rounded(px(12.))
        .px(SPACE_4)
        .py(SPACE_3)
        .bg(linear_gradient(
            145.,
            linear_color_stop(top_bg, 0.),
            linear_color_stop(bottom_bg, 1.),
        ))
        .border(theme::border())
        .border_color(edge)
        .shadow(theme::shadow_small())
        .flex()
        .flex_col()
        .gap(SPACE_3)
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(SPACE_2)
                        .child(
                            div()
                                .w(px(26.))
                                .h(px(18.))
                                .rounded(px(4.))
                                .bg(chip_bg)
                                .border(theme::border())
                                .border_color(edge),
                        )
                        .children(label.map(|l| {
                            div()
                                .text_size(theme::ui_text(TEXT_XS))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(muted)
                                .child(l.to_owned())
                        })),
                )
                .child(card_brand_mark(brand, cx)),
        )
        .child(
            div()
                .font_family(theme::mono_font())
                .text_size(theme::ui_text(15.))
                .letter_spacing(0.12)
                .text_color(ink)
                .child(masked_number.to_owned()),
        )
        .child(
            div()
                .flex()
                .items_end()
                .justify_between()
                .gap(SPACE_3)
                .child(
                    div()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(SPACE_0)
                        .child(
                            div()
                                .text_size(theme::ui_text(9.))
                                .letter_spacing(0.1)
                                .text_color(muted)
                                .child(t!("CARDHOLDER")),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(theme::ui_text(TEXT_2XS))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(ink.opacity(0.86))
                                .child(holder),
                        ),
                )
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .flex_col()
                        .items_end()
                        .gap(SPACE_0)
                        .child(
                            div()
                                .text_size(theme::ui_text(9.))
                                .letter_spacing(0.1)
                                .text_color(muted)
                                .child(t!("EXPIRES")),
                        )
                        .child(
                            div()
                                .font_family(theme::mono_font())
                                .text_size(theme::ui_text(TEXT_2XS))
                                .text_color(ink.opacity(0.86))
                                .child(exp),
                        ),
                ),
        )
        .when(!t.dark, |card| card.shadow(theme::shadow_medium()))
}

pub fn honest_privacy_badge(source: &VaultSource, cx: &App) -> Div {
    let t = Tokens::get(cx);
    let (mark, copy): (AnyElement, &'static str) = match source {
        VaultSource::Sikemux => (
            ui::icon(Icon::ShieldCheck, 13.)
                .text_color(t.live)
                .into_any_element(),
            t!("Encrypted locally with AES-256-GCM. Browser blind-fill keeps values hidden from agents; shell commands and .env files remain readable to local processes."),
        ),
        VaultSource::OnePassword { .. } => (
            ui::picture(Icon::OnePassword, 14., t.accent),
            t!("Stays in 1Password and resolves on demand via op://. Browser blind-fill hides values from agents; shell commands and .env files remain readable to local processes."),
        ),
    };
    div()
        .w_full()
        .px(SPACE_3)
        .py(SPACE_2)
        .rounded(theme::radius_2())
        .bg(t.surface_sunken)
        .border(theme::border())
        .border_color(t.border)
        .flex()
        .items_start()
        .gap(SPACE_2)
        .child(div().pt(px(1.)).flex_none().child(mark))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(theme::ui_text(TEXT_2XS))
                .line_height(theme::ui_text(16.))
                .text_color(t.text_secondary)
                .child(copy),
        )
}

pub fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{} KB", (bytes / 1024).max(1))
    } else {
        format!("{:.1} MB", bytes as f64 / (1024. * 1024.))
    }
}

pub fn format_actor(actor: &str) -> String {
    let trimmed = actor.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("person") {
        t!("You").to_owned()
    } else if trimmed.starts_with('@') {
        trimmed.to_owned()
    } else {
        format!("@{trimmed}")
    }
}

pub fn icon_tile(category: VaultCategory, source: &VaultSource, size: f32, cx: &App) -> Div {
    let t = Tokens::get(cx);
    let color = category_color(category, t);
    let inner: AnyElement = match source {
        VaultSource::OnePassword { .. } => ui::picture(Icon::OnePassword, size * 0.56, t.accent),
        VaultSource::Sikemux => ui::icon(category_icon(category), size * 0.52)
            .text_color(color)
            .into_any_element(),
    };
    div()
        .size(px(size))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme::radius_2())
        .bg(color.opacity(0.09))
        .border(theme::border())
        .border_color(color.opacity(0.22))
        .child(inner)
}

pub fn field_label(label: impl Into<SharedString>, hint: Option<&'static str>, cx: &App) -> Div {
    let t = Tokens::get(cx);
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(SPACE_2)
        .child(
            div()
                .text_size(theme::ui_text(TEXT_XS))
                .font_weight(FontWeight::MEDIUM)
                .text_color(t.text_secondary)
                .child(label.into()),
        )
        .children(hint.map(|h| {
            div()
                .text_size(theme::ui_text(TEXT_3XS))
                .text_color(t.text_tertiary)
                .child(h)
        }))
}

pub fn whisper(text: impl Into<SharedString>, cx: &App) -> Div {
    let t = Tokens::get(cx);
    div()
        .w_full()
        .px(SPACE_2)
        .py(SPACE_1H)
        .rounded(theme::radius_2())
        .bg(t.surface_sunken)
        .border(theme::border())
        .border_color(t.border)
        .flex()
        .items_center()
        .gap(SPACE_1H)
        .text_size(theme::ui_text(TEXT_2XS))
        .text_color(t.text_tertiary)
        .child(ui::icon(Icon::EyeOff, 12.).text_color(t.text_tertiary))
        .child(div().flex_1().min_w_0().truncate().child(text.into()))
}

pub fn empty_state(icon: Icon, title: &'static str, body: &'static str, cx: &App) -> Div {
    let t = Tokens::get(cx);
    div()
        .w_full()
        .px(SPACE_4)
        .py(px(28.))
        .flex()
        .flex_col()
        .items_center()
        .gap(SPACE_1H)
        .child(
            div()
                .size(px(34.))
                .rounded(theme::radius_2())
                .bg(t.surface_raised)
                .border(theme::border())
                .border_color(t.border)
                .flex()
                .items_center()
                .justify_center()
                .child(ui::icon(icon, 16.).text_color(t.text_tertiary)),
        )
        .child(
            div()
                .text_size(theme::ui_text(TEXT_SM))
                .font_weight(FontWeight::MEDIUM)
                .text_color(t.text_primary)
                .child(title),
        )
        .child(
            div()
                .max_w(px(420.))
                .text_center()
                .text_size(theme::ui_text(TEXT_XS))
                .text_color(t.text_tertiary)
                .child(body),
        )
}
