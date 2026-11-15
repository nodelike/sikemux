use super::Zone;

extern "C" {
    #[link_name = "_mktime64"]
    fn mktime(tm: *mut libc::tm) -> libc::time_t;
    #[link_name = "_mkgmtime64"]
    fn mkgmtime(tm: *mut libc::tm) -> libc::time_t;
}

/// `tm` read as a time in `zone`, normalised in place.
pub fn seconds(tm: &mut libc::tm, zone: Zone) -> libc::time_t {
    // SAFETY: `tm` is a live value for the length of the call.
    unsafe {
        match zone {
            Zone::Host => mktime(tm),
            Zone::Utc => mkgmtime(tm),
        }
    }
}

pub fn calendar(seconds: libc::time_t, zone: Zone) -> Option<libc::tm> {
    // SAFETY: `tm` is plain data, for which all zeroes is a valid value.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers point at live values for the length of the call.
    let failed = unsafe {
        match zone {
            Zone::Host => libc::localtime_s(&mut tm, &seconds),
            Zone::Utc => libc::gmtime_s(&mut tm, &seconds),
        }
    };
    (failed == 0).then_some(tm)
}
