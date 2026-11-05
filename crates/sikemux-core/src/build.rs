//! Which build this binary is.

use sikemux_wire::protocol::BuildIdentity;

/// Makes a build pass for another one, so tests can upgrade a core to the
/// binary it already runs: the value replaces the commit and the source, and
/// the build time is zero.
pub const BUILD_ID_OVERRIDE_ENV: &str = "SIKEMUX_BUILD_ID_OVERRIDE";

pub fn identity(version: &str, commit: &str, built_at: u64, source: &str) -> BuildIdentity {
    match std::env::var(BUILD_ID_OVERRIDE_ENV) {
        Ok(id) if !id.is_empty() => BuildIdentity {
            version: version.into(),
            commit: id.clone(),
            built_at: 0,
            source: id,
        },
        _ => BuildIdentity {
            version: version.into(),
            commit: commit.into(),
            built_at,
            source: source.into(),
        },
    }
}
