use super::CoreResult;

#[cfg(windows)]
pub(super) async fn confirmed(what: &str) -> CoreResult<()> {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::OnceLock;

    use sikemux_trust::{Integrity, Token};

    use super::{blocking, CoreError};

    static HIGH: OnceLock<bool> = OnceLock::new();
    static OPEN: AtomicBool = AtomicBool::new(false);
    if !*HIGH.get_or_init(|| matches!(Token::own_integrity(), Ok(Integrity::High))) {
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
