//! Starting programs as the person's own shell would, and the clock, file and
//! lock helpers that work around them.

pub use sikemux_process::{run, ProcessCancellation, ProcessRunError};

pub mod user_environment {
    pub use sikemux_process::user_environment::{command, find_executable, shell_var, var};
}

pub mod clock {
    pub use sikemux_process::clock::{epoch_ms, epoch_secs};
}

pub mod files {
    pub use sikemux_process::files::write_atomic;
}

pub mod sync {
    pub use sikemux_process::sync::Locked;
}
