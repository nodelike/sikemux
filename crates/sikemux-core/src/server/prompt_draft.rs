//! What is typed in the input box of an agent's TUI, read off its screen, so
//! a person moving the agent to its chat keeps what they were writing.

/// How far up from the bottom the input box can sit, below the transcript.
const BOTTOM_ROWS: usize = 20;

/// The text after Claude's input marker. The same marker points at a choice
/// in its menus, so only a line right under a rule counts.
fn prompt(row: &str) -> Option<&str> {
    let after = row.trim_start().strip_prefix('❯')?;
    Some(after.strip_prefix(char::is_whitespace).unwrap_or(after))
}

fn rule(row: &str) -> bool {
    let row = row.trim();
    row.chars().all(|character| matches!(character, '─' | '━')) && row.chars().count() >= 8
}

/// The draft in the input box among the screen's `rows`; empty when there is none.
pub(crate) fn read(rows: &[&str]) -> String {
    let from = rows.len().saturating_sub(BOTTOM_ROWS).max(1);
    let Some(start) = (from..rows.len())
        .rev()
        .find(|&row| prompt(rows[row]).is_some() && rule(rows[row - 1]))
    else {
        return String::new();
    };
    let mut lines = vec![prompt(rows[start]).unwrap_or_default()];
    for row in rows[start + 1..].iter().take_while(|row| !rule(row)) {
        let mut characters = row.chars();
        if !characters.by_ref().take(2).all(char::is_whitespace) {
            return String::new();
        }
        lines.push(characters.as_str());
    }
    let lines: Vec<&str> = lines.iter().map(|line| line.trim_end()).collect();
    lines.join("\n").trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::read;

    #[test]
    fn reads_claudes_input_box_its_continuation_lines_and_not_the_transcript_above() {
        let screen = [
            "❯ fix the login flake",
            "⏺ Done. The test now waits for the page.",
            "",
            "────────────────────────────────────────",
            "❯ now check the logout path too",
            "  and the session timeout",
            "────────────────────────────────────────",
            "  ⏵⏵ bypass permissions on (shift+tab to cycle)",
        ];
        assert_eq!(
            read(&screen),
            "now check the logout path too\nand the session timeout"
        );
    }

    #[test]
    fn reads_claudes_box_as_it_looks_on_screen() {
        let screen = [
            "                                                     ctrl+g to edit in VS Code",
            "────────────────────────────────────────────────",
            "❯ write the migration",
            "  and its rollback",
            "────────────────────────────────────────────────",
            "  ⏵⏵ auto mode on (shift+tab to cycle)",
        ];
        assert_eq!(read(&screen), "write the migration\nand its rollback");
    }

    #[test]
    fn reads_an_empty_box_a_menu_and_a_screen_with_no_box_as_nothing() {
        assert_eq!(read(&["────────────", "❯", "────────────"]), "");
        let trust = [
            "────────────",
            " Accessing workspace:",
            " Security guide",
            " ❯ No, exit",
            "   Yes, I trust this folder",
            " Enter to confirm · Esc to cancel",
        ];
        assert_eq!(read(&trust), "");
        assert_eq!(read(&["$ ls", "README.md  src"]), "");
    }
}
