//! Reads the stack of the thread that crashed out of a native crash's dump, on this machine: the
//! addresses of the code it was running, and which build each of those came from. What the memory
//! held stays here.

use minidump_common::format::{
    CvSignature, MINIDUMP_DIRECTORY, MINIDUMP_EXCEPTION, MINIDUMP_EXCEPTION_STREAM,
    MINIDUMP_HEADER, MINIDUMP_LOCATION_DESCRIPTOR, MINIDUMP_MODULE, MINIDUMP_SIGNATURE,
    MINIDUMP_STREAM_TYPE, MINIDUMP_THREAD,
};
use scroll::{Pread, LE};
use sentry::protocol::debugid::{CodeId, DebugId};
use sentry::protocol::{DebugImage, SymbolicDebugImage};

const MAX_FRAMES: usize = 256;

pub(crate) struct CrashStack {
    /// What the system called the crash, such as `SIGSEGV`.
    pub reason: String,
    /// The addresses the thread was at, innermost first. Each lies inside a loaded image.
    pub addresses: Vec<u64>,
    /// The images those addresses lie in.
    pub images: Vec<DebugImage>,
}

pub(crate) fn read(dump: &[u8]) -> Option<CrashStack> {
    let dump = Dump::read(dump)?;
    let crash: MINIDUMP_EXCEPTION_STREAM = dump.stream(MINIDUMP_STREAM_TYPE::ExceptionStream)?;
    let modules = dump.modules().unwrap_or_default();
    let mut addresses = dump.walk(&crash, &modules).unwrap_or_default();
    addresses.retain(|address| modules.iter().any(|module| module.holds(*address)));
    #[cfg(target_os = "macos")]
    if soft_signal(&crash.exception_record).is_some() {
        strip_signal_handler_frames(&mut addresses, &modules);
    }
    let images = modules
        .iter()
        .filter(|module| addresses.iter().any(|address| module.holds(*address)))
        .filter_map(Module::image)
        .collect();
    Some(CrashStack {
        reason: reason(&crash.exception_record),
        addresses,
        images,
    })
}

struct Dump<'a> {
    bytes: &'a [u8],
    streams: Vec<MINIDUMP_DIRECTORY>,
}

impl<'a> Dump<'a> {
    fn read(bytes: &'a [u8]) -> Option<Self> {
        let header: MINIDUMP_HEADER = bytes.pread_with(0, LE).ok()?;
        if header.signature != MINIDUMP_SIGNATURE {
            return None;
        }
        let mut at = header.stream_directory_rva as usize;
        let streams = (0..header.stream_count)
            .map_while(|_| bytes.gread_with(&mut at, LE).ok())
            .collect();
        Some(Self { bytes, streams })
    }

    fn at(&self, location: MINIDUMP_LOCATION_DESCRIPTOR) -> Option<&'a [u8]> {
        let start = location.rva as usize;
        self.bytes
            .get(start..start.checked_add(location.data_size as usize)?)
    }

    fn stream_bytes(&self, kind: MINIDUMP_STREAM_TYPE) -> Option<&'a [u8]> {
        let stream = (self.streams.iter()).find(|stream| stream.stream_type == kind as u32)?;
        self.at(stream.location)
    }

    fn stream<T: scroll::ctx::TryFromCtx<'a, scroll::Endian, Error = scroll::Error>>(
        &self,
        kind: MINIDUMP_STREAM_TYPE,
    ) -> Option<T> {
        self.stream_bytes(kind)?.pread_with(0, LE).ok()
    }

    /// The entries of a stream that counts them first. Some writers pad the count to eight bytes.
    fn list<T>(&self, kind: MINIDUMP_STREAM_TYPE, entry_size: usize) -> Option<Vec<T>>
    where
        T: scroll::ctx::TryFromCtx<'a, scroll::Endian, Error = scroll::Error>,
    {
        let bytes = self.stream_bytes(kind)?;
        let count = bytes.pread_with::<u32>(0, LE).ok()?;
        let entries = entry_size.checked_mul(count as usize)?;
        let mut at = if bytes.len() == entries.checked_add(8)? {
            8
        } else {
            4
        };
        Some(
            (0..count)
                .map_while(|_| bytes.gread_with(&mut at, LE).ok())
                .collect(),
        )
    }

    fn modules(&self) -> Option<Vec<Module>> {
        let listed: Vec<MINIDUMP_MODULE> =
            self.list(MINIDUMP_STREAM_TYPE::ModuleListStream, MODULE_SIZE)?;
        Some(
            listed
                .into_iter()
                .filter_map(|module| {
                    Some(Module {
                        start: module.base_of_image,
                        size: module.size_of_image.into(),
                        path: self.text(module.module_name_rva)?,
                        build: self.at(module.cv_record).and_then(Build::read),
                    })
                })
                .collect(),
        )
    }

    fn text(&self, at: u32) -> Option<String> {
        let mut at = at as usize;
        let size: u32 = self.bytes.gread_with(&mut at, LE).ok()?;
        let units: Vec<u16> = (0..size / 2)
            .map_while(|_| self.bytes.gread_with(&mut at, LE).ok())
            .collect();
        Some(String::from_utf16_lossy(&units))
    }

    fn walk(&self, crash: &MINIDUMP_EXCEPTION_STREAM, modules: &[Module]) -> Option<Vec<u64>> {
        let threads: Vec<MINIDUMP_THREAD> =
            self.list(MINIDUMP_STREAM_TYPE::ThreadListStream, THREAD_SIZE)?;
        let thread = (threads.iter()).find(|thread| thread.thread_id == crash.thread_id)?;
        let context = [crash.thread_context, thread.thread_context]
            .into_iter()
            .find(|context| context.data_size > 0)?;
        let stack = Stack {
            start: thread.stack.start_of_memory_range,
            bytes: self.at(thread.stack.memory)?,
        };
        Some(walk(Registers::read(self.at(context)?)?, &stack, modules))
    }
}

const MODULE_SIZE: usize = 108;
const THREAD_SIZE: usize = 48;

struct Stack<'a> {
    start: u64,
    bytes: &'a [u8],
}

impl Stack<'_> {
    fn word(&self, address: u64) -> Option<u64> {
        let at = usize::try_from(address.checked_sub(self.start)?).ok()?;
        self.bytes.pread_with(at, LE).ok()
    }
}

struct Module {
    start: u64,
    size: u64,
    path: String,
    build: Option<Build>,
}

impl Module {
    fn holds(&self, address: u64) -> bool {
        (self.start..self.start.saturating_add(self.size)).contains(&address)
    }

    /// The image without its folder, which could name the person.
    fn image(&self) -> Option<DebugImage> {
        let build = self.build.as_ref()?;
        Some(
            SymbolicDebugImage {
                name: file_name(&self.path),
                arch: None,
                image_addr: self.start.into(),
                image_size: self.size,
                image_vmaddr: 0.into(),
                id: build.debug_id,
                code_id: build.code_id.clone(),
                debug_file: build.debug_file.clone(),
            }
            .into(),
        )
    }
}

fn file_name(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_owned()
}

/// Which build of an image was loaded, as its debug files are filed.
struct Build {
    debug_id: DebugId,
    code_id: Option<CodeId>,
    debug_file: Option<String>,
}

impl Build {
    fn read(record: &[u8]) -> Option<Self> {
        let kind: u32 = record.pread_with(0, LE).ok()?;
        if kind == CvSignature::Elf as u32 {
            return Self::elf(record.get(4..)?);
        }
        if kind != CvSignature::Pdb70 as u32 {
            return None;
        }
        let age: u32 = record.pread_with(20, LE).ok()?;
        let file = record.get(24..).unwrap_or_default();
        let file = file.split(|byte| *byte == 0).next().unwrap_or_default();
        Some(Self {
            debug_id: DebugId::from_guid_age(record.get(4..20)?, age).ok()?,
            code_id: None,
            debug_file: Some(file_name(&String::from_utf8_lossy(file)))
                .filter(|file| !file.is_empty()),
        })
    }
}

impl Build {
    fn elf(build_id: &[u8]) -> Option<Self> {
        let mut guid = [0; 16];
        for (byte, known) in guid.iter_mut().zip(build_id) {
            *byte = *known;
        }
        Some(Self {
            debug_id: DebugId::from_guid_age(&guid, 0).ok()?,
            code_id: Some(CodeId::from_binary(build_id)),
            debug_file: None,
        })
    }
}

struct Registers {
    instruction: u64,
    stack: u64,
    frame: u64,
    #[cfg(target_arch = "aarch64")]
    link: u64,
}

impl Registers {
    #[cfg(target_arch = "x86_64")]
    fn read(context: &[u8]) -> Option<Self> {
        let context: minidump_common::format::CONTEXT_AMD64 = context.pread_with(0, LE).ok()?;
        Some(Self {
            instruction: context.rip,
            stack: context.rsp,
            frame: context.rbp,
        })
    }

    #[cfg(target_arch = "aarch64")]
    fn read(context: &[u8]) -> Option<Self> {
        #[cfg(windows)]
        use minidump_common::format::CONTEXT_ARM64 as Context;
        #[cfg(not(windows))]
        use minidump_common::format::CONTEXT_ARM64_OLD as Context;
        let context: Context = context.pread_with(0, LE).ok()?;
        Some(Self {
            instruction: context.pc,
            stack: context.sp,
            frame: context.iregs[29],
            link: context.iregs[30],
        })
    }
}

/// The code on Apple and Windows Arm machines keeps each caller's frame where its own frame starts.
#[cfg(all(target_arch = "aarch64", not(target_os = "linux")))]
fn walk(registers: Registers, stack: &Stack<'_>, _: &[Module]) -> Vec<u64> {
    let mut addresses = vec![registers.instruction];
    let link = code_address(registers.link);
    let caller = registers.frame.checked_add(8).and_then(|at| stack.word(at));
    if caller.map(code_address) != Some(link) {
        addresses.push(link);
    }
    let mut frame = registers.frame.max(registers.stack);
    while addresses.len() < MAX_FRAMES {
        let (Some(next), Some(caller)) = (stack.word(frame), stack.word(frame.saturating_add(8)))
        else {
            break;
        };
        addresses.push(code_address(caller));
        if next <= frame {
            break;
        }
        frame = next;
    }
    addresses
}

/// Apple signs a return address in the bits above the 47 that code is loaded in.
#[cfg(all(target_arch = "aarch64", not(target_os = "linux")))]
fn code_address(signed: u64) -> u64 {
    signed & 0x7fff_ffff_ffff
}

#[cfg(any(target_os = "linux", all(windows, target_arch = "x86_64")))]
mod frames;
#[cfg(any(target_os = "linux", all(windows, target_arch = "x86_64")))]
use frames::walk;

#[cfg(target_os = "macos")]
const EXC_SOFTWARE: u32 = 5;
#[cfg(target_os = "macos")]
const EXC_SOFT_SIGNAL: u32 = 0x10003;

#[cfg(target_os = "macos")]
fn soft_signal(record: &MINIDUMP_EXCEPTION) -> Option<i32> {
    if record.exception_code != EXC_SOFTWARE
        || record.exception_flags != EXC_SOFT_SIGNAL
        || record.number_parameters < 3
    {
        return None;
    }
    i32::try_from(*record.exception_information.get(2)?).ok()
}

/// On macOS, `crash-handler` catches `SIGABRT` in a POSIX signal handler and parks the crashing
/// thread on a condvar while the handler thread dumps it. Dropping the frames up through
/// `_sigtramp` leaves the real abort stack at the top so unrelated aborts do not group together.
#[cfg(target_os = "macos")]
fn strip_signal_handler_frames(addresses: &mut Vec<u64>, modules: &[Module]) {
    let Some(platform) = modules
        .iter()
        .find(|module| file_name(&module.path) == "libsystem_platform.dylib")
    else {
        return;
    };
    let Some(local_base) = local_platform_base() else {
        return;
    };
    let sigtramp = addresses.iter().position(|&address| {
        if !platform.holds(address) {
            return false;
        }
        let Some(local_addr) = address
            .checked_sub(platform.start)
            .and_then(|offset| local_base.checked_add(offset))
        else {
            return false;
        };
        symbol_name_at(local_addr).as_deref() == Some("_sigtramp")
    });
    if let Some(at) = sigtramp {
        if at + 1 < addresses.len() {
            addresses.drain(..=at);
        }
    }
}

#[cfg(target_os = "macos")]
fn local_platform_base() -> Option<u64> {
    // SAFETY: `os_unfair_lock_lock` is a null-terminated symbol name in `libsystem_platform.dylib`.
    let symbol = unsafe { libc::dlsym(libc::RTLD_DEFAULT, c"os_unfair_lock_lock".as_ptr()) };
    if symbol.is_null() {
        return None;
    }
    let mut info = std::mem::MaybeUninit::<libc::Dl_info>::zeroed();
    // SAFETY: `symbol` is a valid address returned by `dlsym`, and `info` is writable.
    if unsafe { libc::dladdr(symbol, info.as_mut_ptr()) } == 0 {
        return None;
    }
    // SAFETY: `dladdr` succeeded and initialized `info`.
    let info = unsafe { info.assume_init() };
    (!info.dli_fbase.is_null()).then_some(info.dli_fbase as usize as u64)
}

#[cfg(target_os = "macos")]
pub(crate) fn symbol_name_at(address: u64) -> Option<String> {
    let mut info = std::mem::MaybeUninit::<libc::Dl_info>::zeroed();
    // SAFETY: `dladdr` inspects dyld metadata without dereferencing `address`, and `info` is writable.
    if unsafe { libc::dladdr(address as usize as *const libc::c_void, info.as_mut_ptr()) } == 0 {
        return None;
    }
    // SAFETY: `dladdr` succeeded and initialized `info`.
    let info = unsafe { info.assume_init() };
    if info.dli_sname.is_null() {
        return None;
    }
    // SAFETY: `dladdr` populates `dli_sname` with a valid static C string when non-null.
    let name = unsafe { std::ffi::CStr::from_ptr(info.dli_sname) };
    Some(name.to_string_lossy().into_owned())
}

#[cfg(target_os = "macos")]
fn reason(record: &MINIDUMP_EXCEPTION) -> String {
    if let Some(signal) = soft_signal(record) {
        return unix_signal_name(signal);
    }
    let code = record.exception_code;
    let name = match code {
        1 => "EXC_BAD_ACCESS",
        2 => "EXC_BAD_INSTRUCTION",
        3 => "EXC_ARITHMETIC",
        EXC_SOFTWARE => "EXC_SOFTWARE",
        6 => "EXC_BREAKPOINT",
        10 => "EXC_CRASH",
        11 => "EXC_RESOURCE",
        12 => "EXC_GUARD",
        _ => return format!("exception {code}"),
    };
    name.to_owned()
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn unix_signal_name(signal: i32) -> String {
    let name = match signal {
        libc::SIGILL => "SIGILL",
        libc::SIGTRAP => "SIGTRAP",
        libc::SIGABRT => "SIGABRT",
        libc::SIGBUS => "SIGBUS",
        libc::SIGFPE => "SIGFPE",
        libc::SIGSEGV => "SIGSEGV",
        _ => return format!("signal {signal}"),
    };
    name.to_owned()
}

#[cfg(target_os = "linux")]
fn reason(record: &MINIDUMP_EXCEPTION) -> String {
    unix_signal_name(record.exception_code as i32)
}

#[cfg(windows)]
fn reason(record: &MINIDUMP_EXCEPTION) -> String {
    let code = record.exception_code;
    let name = match code {
        0xc000_0005 => "EXCEPTION_ACCESS_VIOLATION",
        0xc000_001d => "EXCEPTION_ILLEGAL_INSTRUCTION",
        0xc000_0094 => "EXCEPTION_INT_DIVIDE_BY_ZERO",
        0xc000_00fd => "EXCEPTION_STACK_OVERFLOW",
        0xc000_0374 => "STATUS_HEAP_CORRUPTION",
        0xc000_0409 => "STATUS_STACK_BUFFER_OVERRUN",
        0x8000_0003 => "EXCEPTION_BREAKPOINT",
        _ => return format!("exception {code:#x}"),
    };
    name.to_owned()
}

#[cfg(test)]
#[cfg(target_os = "macos")]
mod tests {
    use super::*;

    #[test]
    fn exc_soft_signal_reports_unix_signal_name() {
        let mut info = [0u64; 15];
        info[0] = u64::from(EXC_SOFTWARE);
        info[1] = u64::from(EXC_SOFT_SIGNAL);
        info[2] = libc::SIGABRT as u64;
        let record = MINIDUMP_EXCEPTION {
            exception_code: EXC_SOFTWARE,
            exception_flags: EXC_SOFT_SIGNAL,
            number_parameters: 3,
            exception_information: info,
            ..Default::default()
        };
        assert_eq!(reason(&record), "SIGABRT");

        let plain = MINIDUMP_EXCEPTION {
            exception_code: EXC_SOFTWARE,
            ..Default::default()
        };
        assert_eq!(reason(&plain), "EXC_SOFTWARE");
    }
}
