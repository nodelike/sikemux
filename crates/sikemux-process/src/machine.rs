//! What the operating system says of the whole computer: what it is, how busy its cores are,
//! and how full its memory and disks are.

use std::path::Path;

/// What a computer is, read once.
#[derive(Clone, Debug, Default)]
pub struct About {
    /// What its maker calls it, such as "MacBook Pro (14-inch, M5)" or "Hetzner vServer".
    pub model: Option<String>,
    /// Its system and version, such as "macOS 26.1" or "Ubuntu 24.04".
    pub system: String,
    /// Who made the machine as its firmware says, such as "Hetzner" or "DigitalOcean".
    pub vendor: Option<String>,
    /// The Linux distribution's id, such as "ubuntu".
    pub distribution: Option<String>,
    /// Its NVIDIA cards, such as "1× H100 80GB HBM3".
    pub gpu: Option<String>,
    pub cores: u32,
    pub memory_total: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Memory {
    pub total: u64,
    pub used: u64,
    pub swap_used: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Disk {
    pub total: u64,
    /// What is no longer free for this user to write.
    pub used: u64,
}

pub fn about() -> About {
    let mut about = system::about();
    about.cores = std::thread::available_parallelism().map_or(1, |cores| cores.get()) as u32;
    if about.memory_total == 0 {
        about.memory_total = memory().map_or(0, |memory| memory.total);
    }
    about
}

pub fn memory() -> Option<Memory> {
    system::memory()
}

/// How full the volume `path` is on.
#[cfg(unix)]
// statvfs's fields are of different widths on each system.
#[allow(clippy::unnecessary_cast)]
pub fn disk(path: &Path) -> Option<Disk> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::zeroed();
    // SAFETY: `path` is NUL-terminated and `stats` is a writable statvfs.
    if unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: statvfs filled it when it answered 0.
    let stats = unsafe { stats.assume_init() };
    let block = stats.f_frsize as u64;
    let total = stats.f_blocks as u64 * block;
    Some(Disk {
        total,
        used: total.saturating_sub(stats.f_bavail as u64 * block),
    })
}

#[cfg(not(unix))]
pub fn disk(_path: &Path) -> Option<Disk> {
    None
}

/// The share of every core's time spent busy between one look and the next.
#[derive(Default)]
pub struct CpuMeter {
    last: Option<system::Ticks>,
}

impl CpuMeter {
    /// The busy share since the last look, in thousandths; none on the first.
    pub fn look(&mut self) -> Option<u16> {
        let now = system::ticks()?;
        let share = self.last.map(|last| now.busy_share_since(&last));
        self.last = Some(now);
        share.flatten()
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn non_empty(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

#[cfg(target_os = "macos")]
mod system {
    use std::ffi::{c_char, c_void, CStr};
    use std::mem::size_of;
    use std::sync::LazyLock;

    use super::{non_empty, About, Memory};

    const HOST_CPU_LOAD_INFO: i32 = 3;
    const HOST_VM_INFO64: i32 = 4;
    const UTF8: u32 = 0x0800_0100;

    extern "C" {
        fn mach_host_self() -> u32;
        fn host_statistics(host: u32, flavor: i32, info: *mut i32, count: *mut u32) -> i32;
        fn host_statistics64(host: u32, flavor: i32, info: *mut i32, count: *mut u32) -> i32;
    }

    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IORegistryEntryFromPath(main_port: u32, path: *const c_char) -> u32;
        fn IORegistryEntryCreateCFProperty(
            entry: u32,
            key: *const c_void,
            allocator: *const c_void,
            options: u32,
        ) -> *const c_void;
        fn IOObjectRelease(object: u32) -> i32;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringCreateWithCString(
            allocator: *const c_void,
            text: *const c_char,
            encoding: u32,
        ) -> *const c_void;
        fn CFGetTypeID(object: *const c_void) -> usize;
        fn CFDataGetTypeID() -> usize;
        fn CFDataGetLength(data: *const c_void) -> isize;
        fn CFDataGetBytePtr(data: *const c_void) -> *const u8;
        fn CFRelease(object: *const c_void);
    }

    static HOST: LazyLock<u32> = LazyLock::new(|| {
        // SAFETY: takes no arguments; the port it gives is kept for the life of the process.
        unsafe { mach_host_self() }
    });

    /// The leading fields of `vm_statistics64`, as many as the kernel is asked for.
    #[repr(C, align(8))]
    #[derive(Default)]
    struct VmStatistics {
        free_count: u32,
        active_count: u32,
        inactive_count: u32,
        wire_count: u32,
        counters: [u64; 9],
        purgeable_count: u32,
        speculative_count: u32,
        compressor_counters: [u64; 4],
        compressor_page_count: u32,
        throttled_count: u32,
        external_page_count: u32,
        internal_page_count: u32,
        total_uncompressed_pages_in_compressor: u64,
    }

    #[derive(Clone, Copy)]
    pub(super) struct Ticks([u32; 4]);

    impl Ticks {
        pub(super) fn busy_share_since(&self, last: &Ticks) -> Option<u16> {
            let [user, system, idle, nice] =
                std::array::from_fn(|state| u64::from(self.0[state].wrapping_sub(last.0[state])));
            let busy = user + system + nice;
            let total = busy + idle;
            (total > 0).then(|| (busy * 1_000 / total) as u16)
        }
    }

    pub(super) fn ticks() -> Option<Ticks> {
        let mut ticks = [0u32; 4];
        let mut count = 4u32;
        // SAFETY: `ticks` holds the four counters `count` says, which the kernel fills.
        let status = unsafe {
            host_statistics(
                *HOST,
                HOST_CPU_LOAD_INFO,
                ticks.as_mut_ptr().cast(),
                &mut count,
            )
        };
        (status == 0).then_some(Ticks(ticks))
    }

    /// A sysctl value of plain data, which any bit pattern of its size makes.
    fn sysctl<T: Copy>(name: &CStr) -> Option<T> {
        let mut value = std::mem::MaybeUninit::<T>::zeroed();
        let mut size = size_of::<T>();
        // SAFETY: the name is NUL-terminated and `value` has `size` writable bytes.
        let status = unsafe {
            libc::sysctlbyname(
                name.as_ptr(),
                value.as_mut_ptr().cast(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        // SAFETY: the kernel wrote all of it, and callers only ask for plain data.
        (status == 0 && size == size_of::<T>()).then(|| unsafe { value.assume_init() })
    }

    fn sysctl_text(name: &CStr) -> Option<String> {
        let mut buffer = [0u8; 256];
        let mut size = buffer.len();
        // SAFETY: the name is NUL-terminated and `buffer` has `size` writable bytes.
        let status = unsafe {
            libc::sysctlbyname(
                name.as_ptr(),
                buffer.as_mut_ptr().cast(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        if status != 0 {
            return None;
        }
        let text = CStr::from_bytes_until_nul(&buffer[..size.min(buffer.len())]).ok()?;
        non_empty(&text.to_string_lossy())
    }

    pub(super) fn memory() -> Option<Memory> {
        let mut stats = VmStatistics::default();
        let mut count = (size_of::<VmStatistics>() / size_of::<i32>()) as u32;
        // SAFETY: `stats` has room for the `count` ints the kernel is asked to fill.
        let status = unsafe {
            host_statistics64(*HOST, HOST_VM_INFO64, (&raw mut stats).cast(), &mut count)
        };
        if status != 0 {
            return None;
        }
        // SAFETY: sysconf only reads the system's configuration.
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(0) as u64;
        let app = stats
            .internal_page_count
            .saturating_sub(stats.purgeable_count);
        let pages =
            u64::from(app) + u64::from(stats.wire_count) + u64::from(stats.compressor_page_count);
        let swap = sysctl::<libc::xsw_usage>(c"vm.swapusage");
        Some(Memory {
            total: sysctl::<u64>(c"hw.memsize").unwrap_or(0),
            used: pages * page,
            swap_used: swap.map_or(0, |swap| swap.xsu_used),
        })
    }

    /// What the device tree calls the product, such as "Mac mini (2023)", which only
    /// System Information otherwise knows.
    fn product_name() -> Option<String> {
        // SAFETY: each object made here is released before returning, and the data is only read
        // while it is held.
        unsafe {
            let entry = IORegistryEntryFromPath(0, c"IODeviceTree:/product".as_ptr());
            if entry == 0 {
                return None;
            }
            let key = CFStringCreateWithCString(std::ptr::null(), c"product-name".as_ptr(), UTF8);
            let value = if key.is_null() {
                std::ptr::null()
            } else {
                IORegistryEntryCreateCFProperty(entry, key, std::ptr::null(), 0)
            };
            let mut name = None;
            if !value.is_null() && CFGetTypeID(value) == CFDataGetTypeID() {
                let length = CFDataGetLength(value).max(0) as usize;
                let bytes = std::slice::from_raw_parts(CFDataGetBytePtr(value), length);
                let bytes = bytes.split(|byte| *byte == 0).next().unwrap_or_default();
                name = non_empty(&String::from_utf8_lossy(bytes));
            }
            if !value.is_null() {
                CFRelease(value);
            }
            if !key.is_null() {
                CFRelease(key);
            }
            IOObjectRelease(entry);
            name
        }
    }

    pub(super) fn about() -> About {
        let chip = sysctl_text(c"machdep.cpu.brand_string")
            .map(|chip| chip.trim_start_matches("Apple ").to_owned());
        let model = match (product_name(), chip) {
            (Some(name), Some(chip)) if !name.contains(&chip) => Some(format!("{name}, {chip}")),
            (Some(name), _) => Some(name),
            (None, chip) => sysctl_text(c"hw.model").or(chip),
        };
        let version = sysctl_text(c"kern.osproductversion");
        About {
            model,
            system: version
                .map_or_else(|| "macOS".to_owned(), |version| format!("macOS {version}")),
            vendor: Some("Apple".to_owned()),
            memory_total: sysctl::<u64>(c"hw.memsize").unwrap_or(0),
            ..About::default()
        }
    }
}

#[cfg(target_os = "linux")]
mod system {
    use std::collections::BTreeMap;
    use std::fs;

    use super::{non_empty, About, Memory};

    #[derive(Clone, Copy)]
    pub(super) struct Ticks {
        busy: u64,
        total: u64,
    }

    impl Ticks {
        pub(super) fn busy_share_since(&self, last: &Ticks) -> Option<u16> {
            let busy = self.busy.saturating_sub(last.busy);
            let total = self.total.saturating_sub(last.total);
            (total > 0).then(|| (busy.min(total) * 1_000 / total) as u16)
        }
    }

    /// The first line of `/proc/stat`: user, nice, system, idle, iowait, irq, softirq, steal.
    pub(super) fn ticks() -> Option<Ticks> {
        let stat = fs::read_to_string("/proc/stat").ok()?;
        let line = stat.lines().next()?.strip_prefix("cpu ")?;
        let counts: Vec<u64> = line
            .split_whitespace()
            .take(8)
            .filter_map(|count| count.parse().ok())
            .collect();
        let [user, nice, system, idle, iowait, irq, softirq, steal] = counts[..] else {
            return None;
        };
        let busy = user + nice + system + irq + softirq + steal;
        Some(Ticks {
            busy,
            total: busy + idle + iowait,
        })
    }

    pub(super) fn memory() -> Option<Memory> {
        let info = fs::read_to_string("/proc/meminfo").ok()?;
        let field = |name: &str| -> Option<u64> {
            let line = info.lines().find(|line| line.starts_with(name))?;
            let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
            Some(kib * 1024)
        };
        let total = field("MemTotal:")?;
        Some(Memory {
            total,
            used: total.saturating_sub(field("MemAvailable:")?),
            swap_used: field("SwapTotal:")
                .unwrap_or(0)
                .saturating_sub(field("SwapFree:").unwrap_or(0)),
        })
    }

    fn read(path: &str) -> Option<String> {
        non_empty(fs::read_to_string(path).ok()?.trim_end_matches('\0'))
    }

    /// Firmware that was never filled in by the board maker reports these.
    fn is_placeholder(name: &str) -> bool {
        let name = name.to_ascii_lowercase();
        [
            "to be filled",
            "system product name",
            "system manufacturer",
            "default string",
            "not specified",
            "o.e.m.",
        ]
        .iter()
        .any(|junk| name.contains(junk))
    }

    fn os_release_field<'a>(os_release: &'a str, key: &str) -> Option<&'a str> {
        os_release
            .lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
            .map(|value| value.trim_matches(['"', '\'']))
            .filter(|value| !value.is_empty())
    }

    /// Each NVIDIA card the driver lists, counted by model.
    fn gpus() -> Option<String> {
        let mut models: BTreeMap<String, usize> = BTreeMap::new();
        for card in fs::read_dir("/proc/driver/nvidia/gpus").ok()?.flatten() {
            let Ok(information) = fs::read_to_string(card.path().join("information")) else {
                continue;
            };
            if let Some(model) = information
                .lines()
                .find_map(|line| line.strip_prefix("Model:"))
                .and_then(non_empty)
            {
                *models
                    .entry(model.trim_start_matches("NVIDIA ").to_owned())
                    .or_default() += 1;
            }
        }
        let named: Vec<String> = models
            .into_iter()
            .map(|(model, count)| format!("{count}× {model}"))
            .collect();
        (!named.is_empty()).then(|| named.join(", "))
    }

    pub(super) fn about() -> About {
        let vendor = read("/sys/class/dmi/id/sys_vendor").filter(|vendor| !is_placeholder(vendor));
        let product =
            read("/sys/class/dmi/id/product_name").filter(|product| !is_placeholder(product));
        let model = match (&vendor, product) {
            (Some(vendor), Some(product)) if !product.starts_with(vendor.as_str()) => {
                Some(format!("{vendor} {product}"))
            }
            (_, Some(product)) => Some(product),
            (_, None) => read("/proc/device-tree/model"),
        };
        let os_release = fs::read_to_string("/etc/os-release").unwrap_or_default();
        let system = match (
            os_release_field(&os_release, "NAME"),
            os_release_field(&os_release, "VERSION_ID"),
        ) {
            (Some(name), Some(version)) => format!("{name} {version}"),
            _ => os_release_field(&os_release, "PRETTY_NAME")
                .unwrap_or("Linux")
                .to_owned(),
        };
        About {
            model,
            system,
            vendor,
            distribution: os_release_field(&os_release, "ID").map(str::to_owned),
            gpu: gpus(),
            ..About::default()
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod system {
    use super::{About, Memory};

    #[derive(Clone, Copy)]
    pub(super) struct Ticks;

    impl Ticks {
        pub(super) fn busy_share_since(&self, _last: &Ticks) -> Option<u16> {
            None
        }
    }

    pub(super) fn ticks() -> Option<Ticks> {
        None
    }

    pub(super) fn memory() -> Option<Memory> {
        None
    }

    pub(super) fn about() -> About {
        About {
            system: std::env::consts::OS.to_owned(),
            ..About::default()
        }
    }
}
