//! Features still being tried, kept to risky changes to how Sikemux runs: the updater,
//! the vault, the protocol, terminals, the sandbox. Each is on for staff and off for everyone
//! else, the accounts server can switch it off everywhere, and it goes by its removal date.

use sikemux_view::device::Flag;

#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "the check that each flag goes by its date reads them"
    )
)]
struct Defined {
    flag: Flag,
    /// Whoever removes it once it has proved itself, or turns it off after a crash.
    owner: &'static str,
    /// The day it should be gone by, as YYYY-MM-DD; a check fails once it has passed.
    remove_by: &'static str,
}

const FLAGS: &[Defined] = &[Defined {
    flag: Flag::StaffFeed,
    owner: "@Yubel",
    remove_by: "2027-02-15",
}];

/// What is on for an account: everything for staff, unless the server switched it off.
pub fn resolve(staff: bool, off: &[String]) -> Vec<Flag> {
    FLAGS
        .iter()
        .filter(|defined| staff && !off.iter().any(|name| *name == name_of(defined.flag)))
        .map(|defined| defined.flag)
        .collect()
}

fn name_of(flag: Flag) -> String {
    serde_json::to_value(flag)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_flag_outlives_its_removal_date() {
        let today = time::OffsetDateTime::now_utc().date();
        let day = time::macros::format_description!("[year]-[month]-[day]");
        for defined in FLAGS {
            let by = time::Date::parse(defined.remove_by, day).expect("a removal date");
            assert!(
                today <= by,
                "{:?} was to be removed by {by}: {} removes it, or moves its date with the person's say",
                defined.flag,
                defined.owner,
            );
        }
    }
}
