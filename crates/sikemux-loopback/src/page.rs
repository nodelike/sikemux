//! The pages a browser is shown while a sign-in goes on and once it has ended. Each is one
//! file with its font, style and scripts inside it, and may load nothing else.

use std::sync::LazyLock;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use sha2::{Digest, Sha256};

/// Where the address bar of a page saying a sign-in worked ends up, which Sikemux's own
/// browser closes the tab at.
pub const DONE: &str = "/done";
pub const SIKEMUX_MARK: &str = include_str!("../../../brand/mark/mark-purple.svg");

const LOCKUP: &str = include_str!("../../../brand/wordmark/lockup-purple.svg");
const CHECK: &str = include_str!("../../../app/assets/icons/check.svg");
const CLOSE: &str = include_str!("../../../app/assets/icons/close.svg");
const ADDRESS_SCRIPT: &str = include_str!("page/address.js");
const SCRIPT: &str = concat!(
    include_str!("../../../shared/signin/dots.js"),
    include_str!("page/page.js")
);
const INK: &str = "#e7e5ef";
const CLOSE_THIS: &str = "You can close this tab.";

static STYLE: LazyLock<String> = LazyLock::new(|| {
    let face = |weight: u16, font: &[u8]| {
        format!(
            "@font-face{{font-family:Figtree;font-weight:{weight};src:url(data:font/woff2;base64,{}) format('woff2')}}",
            STANDARD.encode(font)
        )
    };
    [
        face(400, include_bytes!("page/figtree-400.woff2")),
        face(600, include_bytes!("page/figtree-600.woff2")),
        include_str!("page/page.css").to_owned(),
    ]
    .concat()
});
static ALLOWED: LazyLock<String> = LazyLock::new(|| {
    let hash = |text: &str| format!("'sha256-{}'", STANDARD.encode(Sha256::digest(text)));
    format!(
        "default-src 'none'; script-src {} {}; style-src {}; font-src data:; img-src data:; base-uri 'none'; frame-ancestors 'none'",
        hash(ADDRESS_SCRIPT),
        hash(SCRIPT),
        hash(&STYLE)
    )
});

#[derive(Clone, Copy, PartialEq, Eq)]
enum Look {
    Worked,
    Failed,
    Waiting,
}

enum Below {
    BackToSikemux,
    /// Posts one field to a service on its own, with a button in case it does not.
    Post {
        label: String,
        action: String,
        field: &'static str,
        value: String,
    },
    /// A code to compare, and an address on this server that answers `waiting` until the page
    /// should load again.
    Compare {
        code: String,
        poll: String,
        until: String,
    },
}

pub struct Page {
    look: Look,
    icon: &'static str,
    title: String,
    line: String,
    /// Who or what explains a failure, and how.
    reason: Option<(String, String)>,
    below: Below,
}

impl Page {
    fn new(look: Look, icon: &'static str, title: &str, line: &str) -> Self {
        Self {
            look,
            icon,
            title: title.to_owned(),
            line: line.to_owned(),
            reason: None,
            below: Below::BackToSikemux,
        }
    }

    pub fn worked(icon: &'static str, title: &str, line: &str) -> Self {
        Self::new(Look::Worked, icon, title, line)
    }

    pub fn failed(icon: &'static str, title: &str, line: &str) -> Self {
        Self::new(Look::Failed, icon, title, line)
    }

    /// For a browser that came here with nothing this app asked for.
    pub(crate) fn unknown() -> Self {
        Self::failed(
            SIKEMUX_MARK,
            "Sikemux didn't start this sign-in",
            "It may be an older one, so Sikemux ignored it. Start again from Sikemux.",
        )
    }

    /// Adds why it failed, under who or what says so.
    pub fn because(mut self, label: &str, reason: &str) -> Self {
        let mut letters = reason.chars();
        let reason = letters
            .next()
            .map(|first| first.to_uppercase().chain(letters).collect())
            .unwrap_or_default();
        self.reason = Some((label.to_owned(), reason));
        self
    }

    pub fn posting(
        icon: &'static str,
        title: &str,
        line: &str,
        label: &str,
        action: &str,
        (field, value): (&'static str, &str),
    ) -> Self {
        let mut page = Self::new(Look::Waiting, icon, title, line);
        page.below = Below::Post {
            label: label.to_owned(),
            action: action.to_owned(),
            field,
            value: value.to_owned(),
        };
        page
    }

    pub fn comparing(
        icon: &'static str,
        title: &str,
        line: &str,
        code: &str,
        poll: &str,
        until: &str,
    ) -> Self {
        let mut page = Self::new(Look::Waiting, icon, title, line);
        page.below = Below::Compare {
            code: code.to_owned(),
            poll: poll.to_owned(),
            until: until.to_owned(),
        };
        page
    }

    /// The headers the page is served with, its type included.
    pub fn headers(&self) -> [(&'static str, String); 5] {
        let more = match &self.below {
            // GitHub Enterprise may pass the form on to the company's own sign-in.
            Below::Post { .. } => "; form-action https:",
            Below::Compare { .. } => "; form-action 'none'; connect-src 'self'",
            Below::BackToSikemux => "; form-action 'none'",
        };
        [
            ("content-type", "text/html; charset=utf-8".to_owned()),
            ("content-security-policy", format!("{}{more}", *ALLOWED)),
            ("cache-control", "no-store".to_owned()),
            ("referrer-policy", "no-referrer".to_owned()),
            ("x-content-type-options", "nosniff".to_owned()),
        ]
    }

    pub fn html(&self) -> String {
        let (class, badge) = match self.look {
            Look::Worked => ("worked", CHECK),
            Look::Failed => ("failed", CLOSE),
            Look::Waiting => ("waiting", ""),
        };
        let badge = match badge {
            "" => String::new(),
            badge => format!("<i class=badge>{badge}</i>"),
        };
        let at = match self.look {
            Look::Worked => format!(" data-at={DONE}"),
            _ => String::new(),
        };
        let reason = self
            .reason
            .as_ref()
            .map_or_else(String::new, |(label, reason)| {
                format!(
                    "<div class=why><b>{}</b>{}</div>",
                    escape(label),
                    escape(reason)
                )
            });
        let address_script = match self.below {
            // Its address holds only which sign-in it waits for, and it loads again at it.
            Below::Compare { .. } => String::new(),
            _ => format!("<script>{ADDRESS_SCRIPT}</script>"),
        };
        let (poll, below) = match &self.below {
            Below::BackToSikemux => (
                String::new(),
                format!("<a class=go href=\"sikemux://\">Back to Sikemux</a><small>{CLOSE_THIS}</small>"),
            ),
            Below::Post {
                label,
                action,
                field,
                value,
            } => (
                String::new(),
                format!(
                    "<form method=post action=\"{}\"><input type=hidden name={field} value=\"{}\"><button class=go>{}</button></form>",
                    escape(action),
                    escape(value),
                    escape(label)
                ),
            ),
            Below::Compare { code, poll, until } => (
                format!(" data-poll=\"{}\"", escape(poll)),
                format!("<div class=code>{}</div><small>{}</small>", escape(code), escape(until)),
            ),
        };
        let icon = self.icon;
        format!(
            "<!doctype html><html lang=en{at}><head><meta charset=utf-8><meta name=viewport content=\"width=device-width\"><title>{title}</title><link rel=icon href=\"data:image/svg+xml;base64,{favicon}\">{address_script}<style>{style}</style></head><body class={class}{poll}><canvas data-tint=\"{tint}\" aria-hidden=true></canvas><main><div class=mark aria-hidden=true>{icon}{badge}</div><h1>{title}</h1><p>{line}</p>{reason}{below}</main><footer>{LOCKUP}</footer><script type=module>{SCRIPT}</script></body></html>",
            title = escape(&self.title),
            favicon = STANDARD.encode(SIKEMUX_MARK),
            style = *STYLE,
            tint = tint_of(icon),
            line = escape(&self.line),
        )
    }
}

/// How the browser's page reads once a sign-in has ended.
#[derive(Clone, Copy)]
pub struct Endings {
    pub service: &'static str,
    pub icon: &'static str,
    pub worked: Words,
    pub failed: Words,
}

#[derive(Clone, Copy)]
pub struct Words {
    /// Any `{service}` in it becomes the service's name.
    pub title: &'static str,
    pub line: &'static str,
}

impl Endings {
    pub const fn sign_in(service: &'static str, icon: &'static str) -> Self {
        Self {
            service,
            icon,
            worked: Words {
                title: "Signed in to {service}",
                line: "It's ready in Sikemux.",
            },
            failed: Words {
                title: "Couldn't sign in to {service}",
                line: "Nothing was connected. Try again from Sikemux.",
            },
        }
    }

    pub(crate) fn worked_page(&self) -> Page {
        Page::worked(self.icon, &self.title(self.worked), self.worked.line)
    }

    pub(crate) fn failed_page(&self) -> Page {
        Page::failed(self.icon, &self.title(self.failed), self.failed.line)
    }

    fn title(&self, words: Words) -> String {
        words.title.replace("{service}", self.service)
    }
}

/// The first colour an icon is filled with, or the page's own ink for one drawn in the colour
/// around it.
fn tint_of(icon: &str) -> &str {
    icon.split("fill=\"")
        .skip(1)
        .filter_map(|rest| rest.split('"').next())
        .find(|fill| fill.starts_with('#'))
        .unwrap_or(INK)
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
