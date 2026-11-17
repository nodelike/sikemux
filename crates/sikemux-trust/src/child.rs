use std::ffi::OsString;
use std::sync::OnceLock;

use crate::spawn::person_environment;
use crate::token::{Integrity, Token};

/// What a program this process starts runs as, when that differs from this process.
pub struct Child {
    pub token: Token,
    /// The person's sign-in environment with this process's own variables over it, all but
    /// PATH: a core the service starts runs on a fixed one, and the person's finds their programs.
    pub environment: Vec<(OsString, OsString)>,
}

/// Some in a High core, so no program it starts runs at High; None otherwise, where children
/// run as this process does.
///
/// # Errors
///
/// In a High process that cannot make the Medium token, so nothing is started at High.
pub fn child() -> std::io::Result<Option<&'static Child>> {
    static CHILD: OnceLock<Result<Option<Child>, String>> = OnceLock::new();
    CHILD
        .get_or_init(|| match Token::own_integrity() {
            Ok(Integrity::High) => lowered().map(Some).map_err(|error| {
                format!("this process runs at High and cannot lower its children: {error}")
            }),
            _ => Ok(None),
        })
        .as_ref()
        .map(Option::as_ref)
        .map_err(|error| std::io::Error::other(error.clone()))
}

fn lowered() -> std::io::Result<Child> {
    let token = Token::own()?.for_child()?;
    let mut environment = person_environment(&token)?;
    for (name, value) in std::env::vars_os() {
        if name.eq_ignore_ascii_case("PATH") {
            continue;
        }
        environment.retain(|(kept, _)| !kept.eq_ignore_ascii_case(&name));
        environment.push((name, value));
    }
    Ok(Child { token, environment })
}
