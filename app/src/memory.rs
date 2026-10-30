//! How much memory the app holds, for profiling: launched with `SIKEMUX_MEMORY_LOG=<file>`, it
//! appends its footprint and its GPU allocations to that file every 10 seconds.

use std::fs::OpenOptions;
use std::io::Write;
use std::time::Duration;

use gpui_kit::App;

const LOG: &str = "SIKEMUX_MEMORY_LOG";
const PERIOD: Duration = Duration::from_secs(10);

pub fn log(cx: &mut App) {
    let Some(path) = std::env::var_os(LOG) else {
        return;
    };
    let mut file = match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(file) => file,
        Err(error) => {
            eprintln!(
                "sikemux: no memory log at {}: {error}",
                path.to_string_lossy()
            );
            return;
        }
    };
    let executor = cx.background_executor().clone();
    cx.background_executor()
        .spawn(async move {
            loop {
                let _ = writeln!(file, "{}", sample());
                executor.timer(PERIOD).await;
            }
        })
        .detach();
}

fn sample() -> String {
    let mb = |bytes: Option<u64>| {
        bytes.map_or("-".into(), |bytes| {
            format!("{:.1}MB", bytes as f64 / 1_048_576.)
        })
    };
    format!(
        "{} footprint={} gpu={}",
        chrono::Local::now().format("%H:%M:%S"),
        mb(footprint(std::process::id())),
        mb(gpu_allocated()),
    )
}

/// The memory a process has written, counting what macOS compressed or swapped out: what
/// Activity Monitor shows, which resident memory understates once pages are compressed.
#[cfg(target_os = "macos")]
pub fn footprint(pid: u32) -> Option<u64> {
    let mut info = std::mem::MaybeUninit::<libc::rusage_info_v2>::zeroed();
    // SAFETY: the buffer is a rusage_info_v2, the flavor this call fills.
    let read = unsafe {
        libc::proc_pid_rusage(
            pid as libc::c_int,
            libc::RUSAGE_INFO_V2,
            info.as_mut_ptr().cast(),
        )
    };
    // SAFETY: a successful call filled the whole struct.
    (read == 0).then(|| unsafe { info.assume_init() }.ri_phys_footprint)
}

#[cfg(not(target_os = "macos"))]
pub fn footprint(_: u32) -> Option<u64> {
    None
}

/// Everything this process has allocated on the GPU the app draws with.
#[cfg(target_os = "macos")]
fn gpu_allocated() -> Option<u64> {
    use objc2_metal::MTLDevice;
    objc2_metal::MTLCreateSystemDefaultDevice().map(|device| device.currentAllocatedSize() as u64)
}

#[cfg(not(target_os = "macos"))]
fn gpu_allocated() -> Option<u64> {
    None
}
