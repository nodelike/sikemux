//! The words the app says, in the language the person picked, else the system's when the app
//! speaks it, else English. They come from `app/locales/Localizable.xcstrings`, which the build
//! turns into a table per language, sorted for a binary search. English is the key, so English
//! looks nothing up.

use std::fmt::Display;
use std::sync::atomic::{AtomicUsize, Ordering};

include!(concat!(env!("OUT_DIR"), "/strings.rs"));

/// A language the app speaks besides English.
pub struct Language {
    /// Its BCP 47 tag, as the catalog and the preference name it.
    pub code: &'static str,
    /// Its name in itself.
    pub name: &'static str,
    words: &'static [(&'static str, &'static str)],
}

/// The language spoken, as a place in `LANGUAGES`; past its end is English.
static SPOKEN: AtomicUsize = AtomicUsize::new(usize::MAX);

/// `english` in the language spoken.
pub fn tr(english: &str) -> &str {
    let Some(language) = LANGUAGES.get(SPOKEN.load(Ordering::Relaxed)) else {
        return english;
    };
    match language
        .words
        .binary_search_by(|(key, _)| (*key).cmp(english))
    {
        Ok(at) => language.words[at].1,
        Err(_) => english,
    }
}

/// `text` with each `{name}` in it replaced by its value.
pub fn fill(text: &str, values: &[(&str, &dyn Display)]) -> String {
    let mut filled = text.to_owned();
    for (name, value) in values {
        filled = filled.replace(&format!("{{{name}}}"), &value.to_string());
    }
    filled
}

/// Speaks `code`, or the system's first language the app speaks when none is picked, and
/// whether that changed what is spoken.
pub fn speak(code: Option<&str>) -> bool {
    let at = match code {
        Some(code) => position(code),
        None => sys_locale::get_locales().find_map(|locale| {
            let locale = locale.replace('_', "-");
            position(&locale).or_else(|| match locale.split('-').next() {
                Some("zh") => position("zh-Hans"),
                Some("pt") => position("pt-BR"),
                Some(base) => position(base),
                None => None,
            })
        }),
    }
    .unwrap_or(usize::MAX);
    SPOKEN.swap(at, Ordering::Relaxed) != at
}

fn position(code: &str) -> Option<usize> {
    LANGUAGES
        .iter()
        .position(|language| language.code.eq_ignore_ascii_case(code))
}

/// A string in the language spoken; with values, a `String` with each `{name}` filled.
#[macro_export]
macro_rules! t {
    ($text:literal) => {
        $crate::i18n::tr($text)
    };
    ($text:literal, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::i18n::fill(
            $crate::i18n::tr($text),
            &[$((stringify!($name), &$value as &dyn std::fmt::Display)),+],
        )
    };
}

/// Marks a string for the catalog where it cannot be said yet, such as in a constant; whatever
/// shows it says it with `tr`.
#[macro_export]
macro_rules! n {
    ($text:literal) => {
        $text
    };
}
