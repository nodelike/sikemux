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

/// Whether `version` is an older release than `than`. One that is not a version says nothing.
pub fn older_version(version: &str, than: &str) -> bool {
    match (
        semver::Version::parse(version),
        semver::Version::parse(than),
    ) {
        (Ok(version), Ok(than)) => version < than,
        _ => false,
    }
}

/// Whether moving a core from `running` to `build` would go back: to an older release,
/// or to a build of the same release made earlier. A build time of zero says nothing.
pub fn older(build: &BuildIdentity, running: &BuildIdentity) -> bool {
    older_version(&build.version, &running.version)
        || (build.version == running.version
            && build.built_at != 0
            && running.built_at != 0
            && build.built_at < running.built_at)
}
