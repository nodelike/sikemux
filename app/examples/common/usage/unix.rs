/// The time this thread has run, which other processes busy on the machine do not inflate.
pub fn thread_cpu_ms() -> f64 {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: clock_gettime only writes the struct it is handed.
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut time) };
    time.tv_sec as f64 * 1e3 + time.tv_nsec as f64 / 1e6
}

/// The most memory this process has held, in MB.
pub fn peak_resident_mb() -> f64 {
    // SAFETY: getrusage only writes the struct it is handed.
    let usage = unsafe {
        let mut usage = std::mem::zeroed::<libc::rusage>();
        libc::getrusage(libc::RUSAGE_SELF, &mut usage);
        usage
    };
    usage.ru_maxrss as f64 / 1024. / 1024.
}

/// The clock ADEMark's replay server stamps its request log with.
pub fn uptime_ns() -> u64 {
    const CLOCK_UPTIME_RAW: libc::clockid_t = 8;
    extern "C" {
        fn clock_gettime_nsec_np(clock: libc::clockid_t) -> u64;
    }
    // SAFETY: reads a clock, with no pointer passed.
    unsafe { clock_gettime_nsec_np(CLOCK_UPTIME_RAW) }
}
