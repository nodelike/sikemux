//! Tells phones on the same network that this core is here, as a
//! `_sikemux._udp` service under the Mac's own name, with its key in the TXT
//! record. Nothing here is trusted: pairing binds the key into the code check,
//! so a phone that found an impostor's advert learns it before it says
//! anything.

#[cfg(any(target_os = "macos", test))]
pub(crate) const SERVICE: &str = "_sikemux._udp";

/// `core=<key>` as one length-prefixed TXT string.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn txt_record(core_id: &str) -> Vec<u8> {
    let entry = format!("core={core_id}");
    let mut record = Vec::with_capacity(entry.len() + 1);
    record.push(u8::try_from(entry.len()).unwrap_or(u8::MAX));
    record.extend_from_slice(&entry.as_bytes()[..entry.len().min(255)]);
    record
}

#[cfg(target_os = "macos")]
mod native {
    use std::ffi::{c_char, c_void, CString};

    type ServiceRef = *mut c_void;

    // The DNS Service Discovery API, part of libSystem on macOS.
    extern "C" {
        fn DNSServiceRegister(
            service: *mut ServiceRef,
            flags: u32,
            interface_index: u32,
            name: *const c_char,
            regtype: *const c_char,
            domain: *const c_char,
            host: *const c_char,
            port_network_order: u16,
            txt_len: u16,
            txt_record: *const c_void,
            callback: *const c_void,
            context: *mut c_void,
        ) -> i32;
        fn DNSServiceRefDeallocate(service: ServiceRef);
    }

    /// The advert stays up until this is dropped.
    pub(crate) struct Advert(ServiceRef);

    // SAFETY: the reference is only handed back to `DNSServiceRefDeallocate`,
    // once, from whichever thread drops it, which the API allows.
    unsafe impl Send for Advert {}
    // SAFETY: nothing reads the reference through `&Advert`.
    unsafe impl Sync for Advert {}

    impl Drop for Advert {
        fn drop(&mut self) {
            // SAFETY: the reference came from a successful DNSServiceRegister
            // and is deallocated exactly once, here.
            unsafe { DNSServiceRefDeallocate(self.0) };
        }
    }

    pub(crate) fn advertise(core_id: &str, port: u16) -> Option<Advert> {
        let regtype = CString::new(super::SERVICE).ok()?;
        let txt = super::txt_record(core_id);
        let mut service: ServiceRef = std::ptr::null_mut();
        // SAFETY: every pointer is valid for the call; a null name picks the
        // Mac's own name, null domain and host pick the defaults, and a null
        // callback is allowed when no answer is wanted.
        let status = unsafe {
            DNSServiceRegister(
                &mut service,
                0,
                0,
                std::ptr::null(),
                regtype.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                port.to_be(),
                txt.len() as u16,
                txt.as_ptr().cast(),
                std::ptr::null(),
                std::ptr::null_mut(),
            )
        };
        (status == 0 && !service.is_null()).then_some(Advert(service))
    }
}

#[cfg(target_os = "macos")]
pub(crate) use native::{advertise, Advert};

#[cfg(not(target_os = "macos"))]
pub(crate) struct Advert;

#[cfg(not(target_os = "macos"))]
pub(crate) fn advertise(_core_id: &str, _port: u16) -> Option<Advert> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_txt_record_is_one_length_prefixed_entry() {
        let record = txt_record("abc");
        assert_eq!(record, b"\x08core=abc");
    }
}
