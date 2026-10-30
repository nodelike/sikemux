//! How much drawing work the transcript and its host did, for the stress example to name what a
//! slow frame spent its time on. Only builds with the `counts` feature count; others hold none.

use std::sync::atomic::{AtomicU64, Ordering};

macro_rules! counters {
    ($($name:ident: $doc:literal),* $(,)?) => {
        $(#[doc = $doc] pub static $name: AtomicU64 = AtomicU64::new(0);)*

        /// Every counter's name and value now.
        pub fn now() -> Vec<(&'static str, u64)> {
            vec![$((stringify!($name), $name.load(Ordering::Relaxed))),*]
        }
    };
}

counters! {
    ROWS: "Rows drawn.",
    TEXTS: "Texts laid out whole.",
    CODE_CHUNKS: "Parts of code blocks laid out.",
    CODE_LINES: "Lines of code blocks laid out.",
    CODE_US: "Microseconds spent laying out parts of code blocks.",
    NEW_CODE_LINES: "Lines of code blocks laid out for the first time.",
    NEW_CODE_US: "Microseconds spent laying out parts of code blocks for the first time.",
    UNCOLOURED: "Code blocks drawn before their colours were ready.",
    COLOURED: "Code blocks whose colours arrived.",
    TRIMS: "Times a chat dropped rows it held.",
}

pub fn add(counter: &AtomicU64, by: u64) {
    counter.fetch_add(by, Ordering::Relaxed);
}
