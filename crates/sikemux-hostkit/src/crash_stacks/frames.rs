//! Walks a stack whose frames are laid out as each image's unwind tables say, which are read from
//! the images the stack passes through.

use super::{Build, Module, Registers, Stack, MAX_FRAMES};
use framehop::{
    CacheNative, FrameAddress, MayAllocateDuringUnwind, ModuleSectionInfo, UnwindRegsNative,
    Unwinder, UnwinderNative,
};
use object::{Object, ObjectSection};
use sentry::protocol::debugid::DebugId;

pub(super) fn walk(registers: Registers, stack: &Stack<'_>, modules: &[Module]) -> Vec<u64> {
    let mut unwinder = UnwinderNative::<Vec<u8>, MayAllocateDuringUnwind>::new();
    let mut cache = CacheNative::new_in();
    let mut opened = vec![false; modules.len()];
    let mut addresses = vec![registers.instruction];
    let known = (modules.iter()).any(|module| module.holds(registers.instruction));
    let called = (!known).then(|| called_from(&registers, stack)).flatten();
    let (mut address, mut unwound) = match called {
        Some((caller, unwound)) => {
            let Some(address) = FrameAddress::from_return_address(caller) else {
                return addresses;
            };
            addresses.push(caller);
            (address, unwound)
        }
        None => (
            FrameAddress::from_instruction_pointer(registers.instruction),
            start(&registers),
        ),
    };
    while addresses.len() < MAX_FRAMES {
        let at = address.address_for_lookup();
        if let Some(holder) = modules.iter().position(|module| module.holds(at)) {
            if !std::mem::replace(&mut opened[holder], true) {
                if let Some(module) = modules[holder].unwind_tables() {
                    unwinder.add_module(module);
                }
            }
        }
        let mut word = |at| stack.word(at).ok_or(());
        let Ok(Some(caller)) = unwinder.unwind_frame(address, &mut unwound, &mut cache, &mut word)
        else {
            break;
        };
        let Some(next) = FrameAddress::from_return_address(caller) else {
            break;
        };
        addresses.push(caller);
        address = next;
    }
    addresses
}

#[cfg(target_arch = "x86_64")]
fn start(registers: &Registers) -> UnwindRegsNative {
    UnwindRegsNative::new(registers.instruction, registers.stack, registers.frame)
}

/// Linux signs a return address in the bits above the 48 that code is loaded in.
#[cfg(target_arch = "aarch64")]
fn start(registers: &Registers) -> UnwindRegsNative {
    UnwindRegsNative::new_with_ptr_auth_mask(
        framehop::aarch64::PtrAuthMask(0xffff_ffff_ffff),
        registers.link,
        registers.stack,
        registers.frame,
    )
}

/// A call to an address that holds no code crashes before it has a frame, so the stack is read
/// from its caller.
#[cfg(target_arch = "x86_64")]
fn called_from(registers: &Registers, stack: &Stack<'_>) -> Option<(u64, UnwindRegsNative)> {
    let caller = stack.word(registers.stack)?;
    let above = registers.stack.checked_add(8)?;
    Some((
        caller,
        UnwindRegsNative::new(caller, above, registers.frame),
    ))
}

#[cfg(target_arch = "aarch64")]
fn called_from(registers: &Registers, _: &Stack<'_>) -> Option<(u64, UnwindRegsNative)> {
    Some((registers.link & 0xffff_ffff_ffff, start(registers)))
}

impl Module {
    /// The tables of the file the image was loaded from, unless an update has replaced it since.
    fn unwind_tables(&self) -> Option<framehop::Module<Vec<u8>>> {
        let bytes = std::fs::read(&self.path).ok()?;
        let file = object::File::parse(&*bytes).ok()?;
        if debug_id(&file)? != self.build.as_ref()?.debug_id {
            return None;
        }
        Some(framehop::Module::new(
            self.path.clone(),
            self.start..self.start.saturating_add(self.size),
            self.start,
            Image(file),
        ))
    }
}

fn debug_id(file: &object::File<'_>) -> Option<DebugId> {
    if let Some(build_id) = file.build_id().ok()? {
        return Build::elf(build_id).map(|build| build.debug_id);
    }
    let built = file.pdb_info().ok()??;
    DebugId::from_guid_age(&built.guid(), built.age()).ok()
}

struct Image<'a>(object::File<'a>);

impl ModuleSectionInfo<Vec<u8>> for Image<'_> {
    fn base_svma(&self) -> u64 {
        self.0.relative_address_base()
    }

    fn section_svma_range(&mut self, name: &[u8]) -> Option<std::ops::Range<u64>> {
        let section = self.0.section_by_name_bytes(name)?;
        Some(section.address()..section.address().checked_add(section.size())?)
    }

    fn section_data(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        Some(self.0.section_by_name_bytes(name)?.data().ok()?.to_vec())
    }
}
