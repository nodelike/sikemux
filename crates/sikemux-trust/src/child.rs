use std::ffi::OsString;
use std::sync::OnceLock;

use crate::spawn::person_environment;
use crate::token::{Integrity, Token};

/// What a program this process starts runs as, when that differs from this process.
pub struct Child {
    pub token: Token,
    /// The person's own environment, for a High core that runs on a fixed one.
    pub environment: Vec<(OsString, OsString)>,
}

/// Some in a High core, so no program it starts runs at High; None otherwise, where children
/// run as this process does.
///
/// # Panics
///
/// In a High process that cannot make the Medium token, rather than start anything at High.
pub fn child() -> Option<&'static Child> {
    static CHILD: OnceLock<Option<Child>> = OnceLock::new();
    CHILD
        .get_or_init(|| match Token::own_integrity() {
            Ok(Integrity::High) => Some(lowered().unwrap_or_else(|error| {
                panic!("this process runs at High and cannot lower its children: {error}")
            })),
            _ => None,
        })
        .as_ref()
}

fn lowered() -> std::io::Result<Child> {
    let token = Token::own()?.for_child()?;
    let environment = person_environment(&token)?;
    Ok(Child { token, environment })
}
