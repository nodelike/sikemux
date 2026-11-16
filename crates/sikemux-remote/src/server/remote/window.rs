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

#[cfg(windows)]
pub(super) async fn confirmed(what: &str) -> CoreResult<()> {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::{blocking, CoreError};

    static OPEN: AtomicBool = AtomicBool::new(false);
    if !asks_in_window() {
        return Ok(());
    }
    if OPEN.swap(true, Ordering::AcqRel) {
        return Err(CoreError::from(
            "another request is waiting for your answer in Sikemux's window",
        ));
    }
    let text = format!("{what}\n\nDo you want to allow this?");
    let answered = blocking(move || {
        sikemux_trust::confirm(&text).map_err(|error| CoreError::from(error.to_string()))
    })
    .await;
    OPEN.store(false, Ordering::Release);
    match answered? {
        true => Ok(()),
        false => Err(CoreError::from("you said no in Sikemux's window")),
    }
}

#[cfg(not(windows))]
pub(super) async fn confirmed(_what: &str) -> CoreResult<()> {
    Ok(())
}
