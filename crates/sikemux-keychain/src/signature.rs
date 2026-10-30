//! Whether this program carries the signature of Sikemux's own team.

/// Code Sikemux's team signed, by its Developer ID or development certificate.
pub const TEAM_REQUIREMENT: &str =
    r#"anchor apple generic and certificate leaf[subject.OU] = "D577WD6Z5U""#;

/// Settled once, as the program started, before an update can replace the
/// files it ran from.
pub fn signed_by_team() -> bool {
    static SIGNED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *SIGNED.get_or_init(platform::signed_by_team)
}

#[cfg(target_os = "macos")]
mod platform {
    use security_framework::os::macos::code_signing::{Flags, SecCode, SecRequirement};

    pub(super) fn signed_by_team() -> bool {
        let Ok(code) = SecCode::for_self(Flags::NONE) else {
            return false;
        };
        super::TEAM_REQUIREMENT
            .parse::<SecRequirement>()
            .is_ok_and(|requirement| code.check_validity(Flags::NONE, &requirement).is_ok())
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    pub(super) fn signed_by_team() -> bool {
        false
    }
}
