use serde_json::{json, Value};

/// Every Claude chat is launched with this window. Claude compacts about 33K tokens under it,
/// so near 150K.
pub const CLAUDE_WINDOW: u64 = 183_000;
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

/// What the core asks for when it compacts an idle chat itself.
pub const IDLE_FOCUS: &str = "Keep what the person asked for and decided, and the work still open.";

/// Tokens of context at which Claude compacts a session whose model's window is `size`.
pub fn compact_at(size: u64) -> u64 {
    size.min(CLAUDE_WINDOW).saturating_sub(RESERVE)
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
    })
}
