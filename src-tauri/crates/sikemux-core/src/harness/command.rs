//! Tasks an agent starts from a shell command instead of a `sikemux.json`
//! entry.

pub const COMMAND_TASK_PREFIX: &str = "sh:";

/// The directory inside the project, relative to it, with `.` parts dropped.
pub fn command_cwd(value: Option<&str>) -> Result<String, String> {
    let normalized = value.unwrap_or(".").replace('\\', "/");
    let normalized = normalized.trim_end_matches('/');
    let parts: Vec<&str> = normalized.split('/').filter(|part| *part != ".").collect();
    let drive = normalized
        .as_bytes()
        .get(..2)
        .is_some_and(|head| head[0].is_ascii_alphabetic() && head[1] == b':');
    if normalized.starts_with('/')
        || normalized.starts_with('~')
        || drive
        || parts.iter().any(|part| part.is_empty() || *part == "..")
    {
        return Err("cwd must be a directory inside the project, relative to it".into());
    }
    Ok(parts.join("/"))
}

/// The id comes from the command and its directory, so starting the same
/// command again while it runs finds that run instead of a second copy.
pub fn command_task_id(command: &str, cwd: &str, label: Option<&str>) -> String {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in cwd.bytes().chain([0]).chain(command.bytes()) {
        hash = (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193);
    }
    let mut slug = String::new();
    for character in label.unwrap_or(command).to_lowercase().chars() {
        if character.is_ascii_lowercase() || character.is_ascii_digit() {
            slug.push(character);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug: String = slug.trim_matches('-').chars().take(32).collect();
    let slug = slug.trim_end_matches('-');
    let hex = format!("{hash:08x}");
    format!(
        "{COMMAND_TASK_PREFIX}{}-{}",
        if slug.is_empty() { "command" } else { slug },
        hex.get(..6).unwrap_or(&hex)
    )
}

pub fn command_label(command: &str) -> String {
    if command.chars().count() > 80 {
        format!("{}…", command.chars().take(79).collect::<String>())
    } else {
        command.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_match_the_ones_the_window_used_to_make() {
        assert_eq!(command_task_id("pnpm dev", "", None), "sh:pnpm-dev-b63b1c");
        assert_eq!(
            command_task_id("pnpm dev", "web", Some("Web server!")),
            "sh:web-server-9a37b9"
        );
        assert_eq!(command_task_id("!!!", "", None), "sh:command-aed73e");
        assert_eq!(
            command_task_id(
                "A very long command name that goes past thirty-two chars",
                "",
                None
            ),
            "sh:a-very-long-command-name-that-go-6d9cb5"
        );
    }

    #[test]
    fn a_cwd_must_stay_inside_the_project() {
        assert_eq!(command_cwd(None).unwrap(), "");
        assert_eq!(command_cwd(Some("./web/")).unwrap(), "web");
        assert_eq!(command_cwd(Some("a\\b")).unwrap(), "a/b");
        for outside in ["/etc", "~/x", "C:/x", "../x", "a//b", "a/../b"] {
            assert!(command_cwd(Some(outside)).is_err(), "{outside}");
        }
    }

    #[test]
    fn long_commands_get_a_shortened_label() {
        assert_eq!(command_label("ls"), "ls");
        let long = "x".repeat(90);
        assert_eq!(command_label(&long).chars().count(), 80);
    }
}
