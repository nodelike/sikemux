//! The async runtime an engine's own tasks run on, such as the browser's or
//! the simulator's. The app hands its runtime over once, since their work can
//! start on any thread, the main one included.

use std::future::Future;
use std::sync::OnceLock;

use tokio::runtime::Handle;

static RUNTIME: OnceLock<Handle> = OnceLock::new();

pub fn set(handle: Handle) {
    let _ = RUNTIME.set(handle);
}

pub fn spawn<F>(work: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    match RUNTIME.get() {
        Some(runtime) => {
            runtime.spawn(work);
        }
        None => {
            tokio::spawn(work);
        }
    }
}

/// Waits on `work` from a thread that is not one of the runtime's own.
pub fn block_on<F: Future>(work: F) -> F::Output {
    match RUNTIME.get() {
        Some(runtime) => runtime.block_on(work),
        None => tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime for one call")
            .block_on(work),
    }
}
