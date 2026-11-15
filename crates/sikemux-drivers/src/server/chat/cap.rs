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

/// What the core asks for when it compacts an idle chat itself.
pub const IDLE_FOCUS: &str = "Keep what the person asked for and decided, and the work still open.";

/// Tokens of context at which Claude compacts a session whose model's window is `size`.
pub fn compact_at(size: u64) -> u64 {
    size.min(CLAUDE_WINDOW).saturating_sub(RESERVE)
}

/// The hooks a Claude chat registers as it starts. Every chat is told what to keep before a
/// compaction and handed Sikemux's words back after it; only one that `rotates` has its tools
/// refused while it writes a handoff note.
pub fn claude_hooks(rotates: bool) -> Value {
    let mut hooks = json!({
        "PreCompact": [{ "hookCallbackIds": [KEEP_HOOK] }],
        "SessionStart": [{ "matcher": "compact", "hookCallbackIds": [AFTER_COMPACT_HOOK] }],
    });
    if rotates {
        hooks["PreToolUse"] = json!([{ "hookCallbackIds": [TOOL_HOOK] }]);
    }
    hooks
}
