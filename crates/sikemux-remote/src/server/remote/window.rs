use super::CoreResult;

/// Whether the person answers this core's signing requests in its own window: on Windows
/// when the core runs at High, so none of the person's other programs can answer for them.
#[cfg(windows)]
pub fn asks_in_window() -> bool {
    use std::sync::OnceLock;

    use sikemux_trust::{Integrity, Token};

    static HIGH: OnceLock<bool> = OnceLock::new();
    *HIGH.get_or_init(|| matches!(Token::own_integrity(), Ok(Integrity::High)))
}

#[cfg(not(windows))]
pub fn asks_in_window() -> bool {
    false
}

pub(super) async fn confirmed(what: &str) -> CoreResult<()> {
    if !asks_in_window() {
        return Ok(());
    }
    said(answered(format!("{what}\n\nDo you want to allow this?")).await?)
}

pub(super) fn said(yes: bool) -> CoreResult<()> {
    match yes {
        true => Ok(()),
        false => Err(super::CoreError::from("you said no in Sikemux's window")),
    }
}

/// Shows `secret` in this core's own window, and whether the person said they kept it. Only a
/// core that asks in its window may show one.
pub async fn secret_kept(what: &str, secret: &str) -> CoreResult<bool> {
    if !asks_in_window() {
        return Err(super::CoreError::from(
            "this computer has no window of the core's own to show it in",
        ));
    }
    answered(format!("{what}\n\n{secret}\n\nHave you saved it?")).await
}

#[cfg(windows)]
async fn answered(text: String) -> CoreResult<bool> {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::{blocking, CoreError};

    static OPEN: AtomicBool = AtomicBool::new(false);
    if OPEN.swap(true, Ordering::AcqRel) {
        return Err(CoreError::from(
            "another request is waiting for your answer in Sikemux's window",
        ));
    }
    let answered = blocking(move || {
        sikemux_trust::confirm(&text).map_err(|error| CoreError::from(error.to_string()))
    })
    .await;
    OPEN.store(false, Ordering::Release);
    answered
}

#[cfg(not(windows))]
async fn answered(_text: String) -> CoreResult<bool> {
    unreachable!("only a core that asks in its own window opens one")
}
