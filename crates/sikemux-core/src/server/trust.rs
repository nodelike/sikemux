//! What the core says about how far the person's other programs can reach into it: the
//! integrity it runs at, where each library it loaded came from, and which environment names
//! it was started with. Names only, never their values.

use super::{CoreError, CoreResult};

#[cfg(windows)]
#[derive(serde::Serialize)]
pub(super) struct TrustReport {
    integrity: &'static str,
    libraries: Vec<std::path::PathBuf>,
    environment: Vec<String>,
}

#[cfg(windows)]
pub(super) fn report() -> CoreResult<TrustReport> {
    let failed = |error: std::io::Error| CoreError::from(error.to_string());
    let integrity = match sikemux_trust::Token::own_integrity().map_err(failed)? {
        sikemux_trust::Integrity::Medium => "medium",
        sikemux_trust::Integrity::High => "high",
    };
    let mut environment: Vec<String> = std::env::vars_os()
        .map(|(name, _)| name.to_string_lossy().into_owned())
        .collect();
    environment.sort_unstable();
    Ok(TrustReport {
        integrity,
        libraries: sikemux_trust::loaded_libraries().map_err(failed)?,
        environment,
    })
}

#[cfg(not(windows))]
pub(super) fn report() -> CoreResult<()> {
    Err(CoreError::from("only a Windows core reports its integrity"))
}
