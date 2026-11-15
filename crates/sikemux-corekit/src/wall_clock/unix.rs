use super::Zone;

/// `tm` read as a time in `zone`, normalised in place.
pub fn seconds(tm: &mut libc::tm, zone: Zone) -> libc::time_t {
    // SAFETY: `tm` is a live value for the length of the call.
    unsafe {
        match zone {
            Zone::Host => libc::mktime(tm),
            Zone::Utc => libc::timegm(tm),
        }
    }
}

pub fn calendar(seconds: libc::time_t, zone: Zone) -> Option<libc::tm> {
    // SAFETY: `tm` is plain data, for which all zeroes is a valid value.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers point at live values for the length of the call.
    let filled = unsafe {
        match zone {
            Zone::Host => libc::localtime_r(&seconds, &mut tm),
            Zone::Utc => libc::gmtime_r(&seconds, &mut tm),
        }
    };
    (!filled.is_null()).then_some(tm)
}
