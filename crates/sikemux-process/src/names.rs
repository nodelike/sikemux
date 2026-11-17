//! What the build calls its programs and the app, wherever one of them looks for another.

/// The window's program, which is also the command agents run.
pub const APP_PROGRAM: &str = "sikemux";
pub const CORE_PROGRAM: &str = "sikemux-host";
pub const APP_EXECUTABLE: &str = if cfg!(windows) {
    "sikemux.exe"
} else {
    APP_PROGRAM
};
pub const CORE_EXECUTABLE: &str = if cfg!(windows) {
    "sikemux-host.exe"
} else {
    CORE_PROGRAM
};
/// The app's identifier, with `.dev` for dev builds.
pub const APP_IDENTIFIER: &str = if cfg!(debug_assertions) {
    "com.nodelike.sikemux.dev"
} else {
    "com.nodelike.sikemux"
};
