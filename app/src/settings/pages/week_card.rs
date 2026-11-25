//! The picture of the last seven days that Settings ▸ Usage shares: which models did the work and
//! in what share, the commits and the agent hours, drawn on Sikemux's black and purple.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::{Arc, LazyLock};

use chrono::{Datelike, Duration, Local, NaiveTime, TimeZone};
use resvg::{tiny_skia, usvg};
use sikemux_calls::activity_log::ActivitySummary;

use crate::icons::{self, Icon};
use crate::reads::Catalog;
use crate::theme;

/// The card's own size; the saved picture is twice it.
const WIDTH: f32 = 1200.;
const HEIGHT: f32 = 675.;
pub const SAVED_SCALE: f32 = 2.;
pub const PREVIEW_SCALE: f32 = 632. / WIDTH;

/// Families under this share of the week join "other".
const SMALLEST_PERCENT: u64 = 3;
const MOST_SLICES: usize = 5;
const OTHER_COLOR: &str = "#4a4658";

const BAR_LEFT: f32 = 64.;
const BAR_WIDTH: f32 = WIDTH - 2. * BAR_LEFT;
const BAR_GAP: f32 = 6.;
const KEY_GAP: f32 = 40.;
const KEY_ICON: f32 = 22.;
const KEY_ICON_GAP: f32 = 10.;
const KEY_TEXT: f32 = 20.;

const LOCKUP: &str = include_str!("../../../../brand/wordmark/lockup-white.svg");

/// One slice of the bar: a model family, or everything too small to name.
#[derive(Clone)]
struct Slice {
    label: String,
    percent: u64,
    color: String,
    icon: Option<Icon>,
}

/// Everything the card says, in the person's language, ready to draw off the main thread.
#[derive(Clone)]
pub struct Card {
    slices: Vec<Slice>,
    lead: String,
    claim: String,
    claim_rest: String,
    footer: Vec<(String, bool)>,
}

/// Local midnight six days ago, so the week ends today, and how the card names that span.
pub fn window() -> (i64, String) {
    let today = Local::now().date_naive();
    let first = today - Duration::days(6);
    let start = Local
        .from_local_datetime(&first.and_time(NaiveTime::MIN))
        .earliest()
        .map_or(0, |at| at.timestamp_millis());
    let range = match first.month() == today.month() {
        true => t!(
            "{first} to {last}",
            first = first.format("%b %-d"),
            last = today.format("%-d")
        ),
        false => t!(
            "{first} to {last}",
            first = first.format("%b %-d"),
            last = today.format("%b %-d")
        ),
    };
    (start, range)
}

impl Card {
    /// The week in `summary`, or none when no agent used a model in it.
    pub fn of(summary: &ActivitySummary, range: &str, catalog: &Catalog) -> Option<Card> {
        let mut families: HashMap<Cow<'static, str>, (Family, u64)> = HashMap::new();
        for model in &summary.models {
            let tokens = (model.input + model.output + model.cache_write).max(0) as u64;
            if tokens == 0 {
                continue;
            }
            let family = family(&model.model, &model.agent, catalog);
            families
                .entry(family.label.clone())
                .or_insert((family, 0))
                .1 += tokens;
        }
        let total: u64 = families.values().map(|(_, tokens)| tokens).sum();
        if total == 0 {
            return None;
        }
        let mut ranked: Vec<(Family, u64)> = families.into_values().collect();
        ranked.sort_by(|a, b| {
            (a.0.icon.is_none().cmp(&b.0.icon.is_none()))
                .then(b.1.cmp(&a.1))
                .then(a.0.label.cmp(&b.0.label))
        });
        let count = ranked.len();
        let named = ranked
            .iter()
            .take(MOST_SLICES)
            .take_while(|(family, tokens)| {
                family.icon.is_some() && tokens * 100 >= SMALLEST_PERCENT * total
            })
            .count();
        let lead_label = ranked
            .first()
            .map_or("", |(family, _)| family.label.as_ref());
        let (kept, folded) = ranked.split_at(named);
        let other: u64 = folded.iter().map(|(_, tokens)| tokens).sum();
        let mut shares: Vec<u64> = kept.iter().map(|(_, tokens)| *tokens).collect();
        if other > 0 {
            shares.push(other);
        }
        let percents = whole_percents(&shares, total);
        let mut slices: Vec<Slice> = kept
            .iter()
            .zip(&percents)
            .map(|((family, _), percent)| Slice {
                label: family.label.as_ref().to_owned(),
                percent: *percent,
                color: family.color.as_ref().to_owned(),
                icon: family.icon,
            })
            .collect();
        if let Some(percent) = percents.get(named).filter(|percent| **percent > 0) {
            slices.push(Slice {
                label: t!("other").to_owned(),
                percent: *percent,
                color: OTHER_COLOR.to_owned(),
                icon: None,
            });
        }
        let (lead, claim, claim_rest) = match count {
            1 => (
                t!("My week in Sikemux:").to_owned(),
                t!("100% {model}", model = lead_label),
                ".".to_owned(),
            ),
            _ => (
                t!("My team this week:").to_owned(),
                t!("{count} models", count = number_word(count)),
                t!(", one window.").to_owned(),
            ),
        };
        Some(Card {
            slices,
            lead,
            claim,
            claim_rest,
            footer: footer(summary, range),
        })
    }
}

/// "41 commits in 38 hours of agent work, Nov 16 to 22", with the counts marked to stand out.
fn footer(summary: &ActivitySummary, range: &str) -> Vec<(String, bool)> {
    let commits = summary.totals.commits;
    let minutes = summary.totals.agent_ms / 60_000;
    let commits = match commits {
        1 => t!("1 commit").to_owned(),
        _ => t!("{count} commits", count = commits),
    };
    let time = match minutes {
        1 => t!("1 minute").to_owned(),
        ..60 => t!("{count} minutes", count = minutes),
        60..120 => t!("1 hour").to_owned(),
        _ => t!("{count} hours", count = minutes / 60),
    };
    vec![
        (commits, true),
        (t!(" in ").to_owned(), false),
        (time, true),
        (t!(" of agent work, {range}", range = range), false),
    ]
}

/// Whole percents of `total` that add up to 100, the leftover points going to the largest
/// remainders.
fn whole_percents(shares: &[u64], total: u64) -> Vec<u64> {
    let mut parts: Vec<(usize, u64, u64)> = (shares.iter().enumerate())
        .map(|(at, share)| (at, share * 100 / total, share * 100 % total))
        .collect();
    let short = 100 - parts.iter().map(|(_, whole, _)| whole).sum::<u64>();
    parts.sort_by_key(|(_, _, remainder)| std::cmp::Reverse(*remainder));
    for (_, whole, _) in parts.iter_mut().take(short as usize) {
        *whole += 1;
    }
    parts.sort_by_key(|(at, _, _)| *at);
    parts.into_iter().map(|(_, whole, _)| whole).collect()
}

fn number_word(count: usize) -> String {
    let word = match count {
        2 => t!("two"),
        3 => t!("three"),
        4 => t!("four"),
        5 => t!("five"),
        6 => t!("six"),
        7 => t!("seven"),
        8 => t!("eight"),
        9 => t!("nine"),
        10 => t!("ten"),
        _ => return count.to_string(),
    };
    word.to_owned()
}

/// Who made a model, as the card names it.
struct Family {
    label: Cow<'static, str>,
    icon: Option<Icon>,
    color: Cow<'static, str>,
}

fn family(model: &str, agent: &str, catalog: &Catalog) -> Family {
    let name = model
        .rsplit('/')
        .next()
        .unwrap_or(model)
        .to_ascii_lowercase();
    let first = name
        .split(['-', '.', ':', '_', ' '])
        .next()
        .unwrap_or_default();
    let known = |label: &'static str, icon, color: &'static str| Family {
        label: Cow::Borrowed(label),
        icon: Some(icon),
        color: Cow::Borrowed(color),
    };
    match first {
        "claude" | "opus" | "sonnet" | "haiku" | "fable" => {
            known("Claude", Icon::Claude, "#d97757")
        }
        "gpt" | "o1" | "o3" | "o4" | "codex" => known("GPT", Icon::OpenAi, "#7a9dff"),
        "gemini" | "gemma" => known("Gemini", Icon::Gemini, "#4285f4"),
        "grok" => known("Grok", Icon::Grok, "#fcfcfc"),
        "deepseek" => known("DeepSeek", Icon::DeepSeek, "#5b8cff"),
        "glm" => known("GLM", Icon::Zhipu, "#a78bfa"),
        "kimi" | "moonshot" => known("Kimi", Icon::Kimi, "#2dd4bf"),
        "qwen" => known("Qwen", Icon::Alibaba, "#615ced"),
        "minimax" => known("MiniMax", Icon::MiniMax, "#f23f5d"),
        "llama" => known("Llama", Icon::Meta, "#0081fb"),
        "mistral" | "codestral" | "devstral" => known("Mistral", Icon::Mistral, "#fa520f"),
        _ => match catalog.kind(agent) {
            Some(entry) => Family {
                label: Cow::Owned(entry.label.clone()),
                icon: Some(icons::agent(agent)),
                color: Cow::Owned(entry.brand.clone()),
            },
            None => Family {
                label: Cow::Borrowed("other"),
                icon: None,
                color: Cow::Borrowed(OTHER_COLOR),
            },
        },
    }
}

/// Figtree, as the app sets its text, then the system's fonts for any letter it lacks.
static FONTS: LazyLock<Arc<usvg::fontdb::Database>> = LazyLock::new(|| {
    let mut fonts = usvg::fontdb::Database::new();
    fonts.load_system_fonts();
    fonts.load_font_data(theme::UI_REGULAR.to_vec());
    fonts.load_font_data(theme::UI_SEMIBOLD.to_vec());
    fonts.set_sans_serif_family(theme::BUNDLED_UI);
    Arc::new(fonts)
});

fn options() -> usvg::Options<'static> {
    usvg::Options {
        font_family: theme::BUNDLED_UI.to_owned(),
        fontdb: FONTS.clone(),
        ..usvg::Options::default()
    }
}

/// The card as a PNG, `scale` times its own size.
pub fn draw(card: &Card, scale: f32) -> Result<Vec<u8>, String> {
    let svg = svg(card, &key_widths(card)?);
    let tree = usvg::Tree::from_str(&svg, &options()).map_err(|fault| fault.to_string())?;
    let (width, height) = (
        (WIDTH * scale).round() as u32,
        (HEIGHT * scale).round() as u32,
    );
    let mut pixmap = tiny_skia::Pixmap::new(width, height).ok_or("The card is empty")?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let pixels = image::RgbaImage::from_raw(width, height, pixmap.take())
        .ok_or("The card could not be made")?;
    let mut png = Vec::new();
    pixels
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|fault| fault.to_string())?;
    Ok(png)
}

/// How wide each key's words are set, measured by laying them out once on their own.
fn key_widths(card: &Card) -> Result<Vec<f32>, String> {
    let mut svg =
        format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="{WIDTH}" height="{HEIGHT}">"#);
    for (at, slice) in card.slices.iter().enumerate() {
        let _ = write!(
            svg,
            r#"<text id="key{at}" x="0" y="100">{}</text>"#,
            key_text(slice)
        );
    }
    svg.push_str("</svg>");
    let tree = usvg::Tree::from_str(&svg, &options()).map_err(|fault| fault.to_string())?;
    Ok((0..card.slices.len())
        .map(|at| {
            tree.node_by_id(&format!("key{at}"))
                .map(|node| node.abs_bounding_box().width())
                .unwrap_or(0.)
        })
        .collect())
}

fn key_text(slice: &Slice) -> String {
    format!(
        r##"<tspan font-size="{KEY_TEXT}" font-weight="600" fill="#f1eff7">{}%</tspan><tspan font-size="{KEY_TEXT}" fill="#b4afc4"> {}</tspan>"##,
        slice.percent,
        escape(&slice.label)
    )
}

fn svg(card: &Card, key_widths: &[f32]) -> String {
    let mut svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{WIDTH}" height="{HEIGHT}" viewBox="0 0 {WIDTH} {HEIGHT}" font-family="{font}">
<defs>
<radialGradient id="glow" cx="0.5" cy="0.5" r="0.5">
<stop offset="0" stop-color="#a277ff" stop-opacity="0.24"/>
<stop offset="0.48" stop-color="#a277ff" stop-opacity="0.09"/>
<stop offset="1" stop-color="#a277ff" stop-opacity="0"/>
</radialGradient>
<filter id="grain" x="0" y="0" width="100%" height="100%">
<feTurbulence type="fractalNoise" baseFrequency="0.9" numOctaves="2" stitchTiles="stitch"/>
<feColorMatrix type="saturate" values="0"/>
</filter>
</defs>
<rect width="{WIDTH}" height="{HEIGHT}" fill="#000"/>
<rect x="120" y="170" width="960" height="480" fill="url(#glow)"/>
<rect width="{WIDTH}" height="{HEIGHT}" filter="url(#grain)" opacity="0.07"/>
"##,
        font = theme::BUNDLED_UI,
    );
    svg.push_str(&nested(LOCKUP, 64., 60., 28. * 758.27 / 157.14, 28., None));
    let _ = write!(
        svg,
        r##"<text x="{}" y="79" text-anchor="end" font-size="18" fill="#7f7a91">sikemux.com</text>"##,
        WIDTH - 64.
    );
    let _ = write!(
        svg,
        r##"<g font-size="64" font-weight="600" letter-spacing="-2.88" fill="#f1eff7"><text x="64" y="212">{}</text><text x="64" y="277"><tspan fill="#b99aff">{}</tspan>{}</text></g>"##,
        escape(&card.lead),
        escape(&card.claim),
        escape(&card.claim_rest),
    );

    let gaps = BAR_GAP * card.slices.len().saturating_sub(1) as f32;
    let mut x = BAR_LEFT;
    for slice in &card.slices {
        let width = (BAR_WIDTH - gaps) * slice.percent as f32 / 100.;
        let _ = write!(
            svg,
            r#"<rect x="{x:.2}" y="368" width="{width:.2}" height="84" rx="6" fill="{}"/>"#,
            slice.color
        );
        x += width + BAR_GAP;
    }

    let widths: Vec<f32> = (card.slices.iter().zip(key_widths))
        .map(|(slice, text)| text + slice.icon.map_or(0., |_| KEY_ICON + KEY_ICON_GAP))
        .collect();
    let spare = BAR_WIDTH - widths.iter().sum::<f32>();
    let gap = match widths.len() {
        0 | 1 => 0.,
        many => (spare / (many - 1) as f32).clamp(16., KEY_GAP),
    };
    let mut x = BAR_LEFT;
    for ((slice, width), text_width) in card.slices.iter().zip(&widths).zip(key_widths) {
        let mut text_x = x;
        if let Some(icon) = slice.icon {
            svg.push_str(&nested(
                std::str::from_utf8(icon.svg()).unwrap_or_default(),
                x,
                474.,
                KEY_ICON,
                KEY_ICON,
                Some(&slice.color),
            ));
            text_x += KEY_ICON + KEY_ICON_GAP;
        }
        let _ = write!(
            svg,
            r#"<text x="{text_x:.2}" y="492">{}</text>"#,
            key_text(slice)
        );
        x += width.max(*text_width) + gap;
    }

    svg.push_str(r##"<text x="64" y="612" font-size="20" fill="#7f7a91">"##);
    for (words, marked) in &card.footer {
        match marked {
            true => {
                let _ = write!(
                    svg,
                    r##"<tspan font-weight="600" fill="#f1eff7">{}</tspan>"##,
                    escape(words)
                );
            }
            false => {
                let _ = write!(svg, "<tspan>{}</tspan>", escape(words));
            }
        }
    }
    svg.push_str("</text></svg>");
    svg
}

/// `file`, a whole SVG, placed in the box at `x`, `y`, its `currentColor` set to `color`.
fn nested(file: &str, x: f32, y: f32, width: f32, height: f32, color: Option<&str>) -> String {
    let Some((_, after)) = file.split_once("<svg") else {
        return String::new();
    };
    let color = color
        .map(|color| format!(r#" color="{color}""#))
        .unwrap_or_default();
    format!(
        r#"<svg x="{x:.2}" y="{y:.2}" width="{width:.2}" height="{height:.2}"{color}{}"#,
        after
    )
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
