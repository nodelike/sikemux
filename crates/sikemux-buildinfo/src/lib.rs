//! Which build this is: its commit, its time and a fingerprint of the core's sources, worked
//! out once by this crate's build script for the `sikemux` command and the app built beside it.
//! Every change to the core's sources compiles this crate again, so only a program's `main`
//! depends on it, never a library.

use sikemux_wire::protocol::BuildIdentity;

/// The commit this build was made from.
pub const COMMIT: &str = env!("SIKEMUX_BUILD_COMMIT");

/// The build of the `sikemux` command and of the app built beside it.
pub fn identity() -> BuildIdentity {
    sikemux_corekit::build::identity(
        env!("CARGO_PKG_VERSION"),
        COMMIT,
        env!("SIKEMUX_BUILD_TIME").parse().unwrap_or(0),
        env!("SIKEMUX_BUILD_SOURCE"),
    )
}
