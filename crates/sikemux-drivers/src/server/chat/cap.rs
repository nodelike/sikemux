use serde_json::{json, Value};

/// Every Claude chat but a Fable one is launched with this window. Claude compacts about 33K
/// tokens under it, so near 150K.
pub const CLAUDE_WINDOW: u64 = 183_000;
/// Antigravity ACP sessions are compacted once a turn finishes at or past this context size.
pub const ANTIGRAVITY_AUTO_COMPACT: u64 = 150_000;
/// Env var honored by Antigravity CLI (ignored by `agy acp` as of 1.5.16, kept for terminal/future ACP).
pub const ANTIGRAVITY_COMPACT_ENV: (&str, &str) = ("AGY_CLI_AUTO_COMPACT_OVERRIDE", "150K");
/// A Fable chat compacts near 250K instead: at 150K it ran out of room twice as fast as Opus,
/// and its cheap cache reads make the larger context cost no more than the extra compactions.
const FABLE_WINDOW: u64 = 283_000;
const RESERVE: u64 = 33_000;
/// The callbacks Claude calls before it compacts and as it starts again after.
pub const KEEP_HOOK: &str = "sikemux-keep";
pub const AFTER_COMPACT_HOOK: &str = "sikemux-after-compact";
/// The callback Claude calls before each tool, so a handoff note runs none, even one the
/// person's settings allow without asking.
pub const TOOL_HOOK: &str = "sikemux-tool";
/// The callback Claude calls before each shell command, so what leaves this machine by a
/// program run by its path, past the shim on PATH, is still held for the person.
pub const SHIP_HOOK: &str = "sikemux-ship";
/// The callback Claude calls after each tool, so parallel bursts and huge reads stay within
/// one turn's output budget while spilling full text to a file.
pub const POST_TOOL_HOOK: &str = "sikemux-post-tool";

/// What the core asks for when it compacts an idle chat itself.
pub const IDLE_FOCUS: &str = "Keep what the person asked for and decided, and the work still open.";

/// The window a Claude chat launched with `model` is given.
pub fn claude_window(model: Option<&str>) -> u64 {
    if model.is_some_and(|model| model.contains("fable")) {
        FABLE_WINDOW
    } else {
        CLAUDE_WINDOW
    }
}

/// Tokens of context at which Claude compacts a session launched with `window` whose model's
/// own window is `size`.
pub fn compact_at(size: u64, window: u64) -> u64 {
    size.min(window).saturating_sub(RESERVE)
}

/// The hooks a Claude chat registers as it starts. Every chat is told what to keep before a
/// compaction, handed Sikemux's words back after it, and has its shell commands checked for
/// what leaves this machine; only one that `rotates` has its tools refused while it writes a
/// handoff note.
pub fn claude_hooks(rotates: bool) -> Value {
    let mut before_tool = vec![json!({ "matcher": "Bash", "hookCallbackIds": [SHIP_HOOK] })];
    if rotates {
        before_tool.push(json!({ "hookCallbackIds": [TOOL_HOOK] }));
    }
    json!({
        "PreCompact": [{ "hookCallbackIds": [KEEP_HOOK] }],
        "SessionStart": [{ "matcher": "compact", "hookCallbackIds": [AFTER_COMPACT_HOOK] }],
        "PreToolUse": before_tool,
        "PostToolUse": [{ "hookCallbackIds": [POST_TOOL_HOOK] }],
    })
}
