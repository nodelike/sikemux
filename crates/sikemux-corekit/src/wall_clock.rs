//! Times as a person or an agent writes them out, such as "Oct 17" or "2:46 pm", read
//! against this host's clock or UTC.

use sikemux_process::clock::epoch_ms;

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// A day an agent writes out, such as "Oct 17" or "Oct 17th", as its month from 1 and day.
pub fn month_day(text: &str) -> Option<(i32, i32)> {
    let (name, day) = text.split_once(' ')?;
    let month = MONTHS.iter().position(|known| *known == name)? as i32 + 1;
    let day = day
        .trim_end_matches(char::is_alphabetic)
        .parse::<i32>()
        .ok()?;
    (1..=31).contains(&day).then_some((month, day))
}

/// `at`, in Unix milliseconds, as this host's clock reads it: "at 15:40"
/// today, else "on 18 Oct at 15:40".
pub fn when(at: u64) -> String {
    let (Some(then), Some(now)) = (calendar(at, Zone::Host), calendar(epoch_ms(), Zone::Host))
    else {
        return String::new();
    };
    let clock = format!("at {:02}:{:02}", then.tm_hour, then.tm_min);
    if (then.tm_year, then.tm_yday) == (now.tm_year, now.tm_yday) {
        return clock;
    }
    let month = MONTHS
        .get(then.tm_mon as usize)
        .copied()
        .unwrap_or_default();
    format!("on {} {month} {clock}", then.tm_mday)
}

/// The zone a reset time an agent writes out is in.
#[derive(Clone, Copy)]
pub enum Zone {
    Host,
    Utc,
}

/// A clock such as `2:46` or `3` as the hour of the day and its minute.
pub fn twelve_hour(clock: &str, pm: bool) -> Option<(i32, i32)> {
    let (hour, minute) = clock.split_once(':').unwrap_or((clock, "0"));
    let (hour, minute): (i32, i32) = (hour.parse().ok()?, minute.parse().ok()?);
    ((1..=12).contains(&hour) && (0..60).contains(&minute))
        .then(|| (hour % 12 + if pm { 12 } else { 0 }, minute))
}

/// A reset time as an agent writes it out, to the minute.
pub struct WallClock {
    pub zone: Zone,
    /// Year, month from 1 and day, when it names a day; the year defaults to this one.
    pub date: Option<(Option<i32>, i32, i32)>,
    pub hour: i32,
    pub minute: i32,
}

impl WallClock {
    /// In Unix milliseconds: on its day, else the next time the clock reads
    /// it. `None` for a day already past.
    pub fn unix_ms(&self) -> Option<u64> {
        let now = epoch_ms();
        match self.on_day(now, 0)? {
            at if at > now => Some(at),
            _ if self.date.is_some() => None,
            _ => self.on_day(now, 1).filter(|at| *at > now),
        }
    }

    pub fn on_day(&self, now: u64, days_later: i32) -> Option<u64> {
        let mut tm = calendar(now, self.zone)?;
        if let Some((year, month, day)) = self.date {
            if let Some(year) = year {
                tm.tm_year = year - 1900;
            }
            tm.tm_mon = month - 1;
            tm.tm_mday = day;
        }
        tm.tm_mday += days_later;
        (tm.tm_hour, tm.tm_min, tm.tm_sec, tm.tm_isdst) = (self.hour, self.minute, 0, -1);
        // SAFETY: `tm` is a live value for the length of the call.
        let at = unsafe {
            match self.zone {
                Zone::Host => libc::mktime(&mut tm),
                Zone::Utc => libc::timegm(&mut tm),
            }
        };
        u64::try_from(at).ok()?.checked_mul(1000)
    }
}

pub fn calendar(at: u64, zone: Zone) -> Option<libc::tm> {
    let seconds = (at / 1000) as libc::time_t;
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
