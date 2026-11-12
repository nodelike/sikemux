use serde_json::{json, Value};

/// Claude compacts about 33K tokens under the window it is given, so this compacts near 150K.
pub const CLAUDE_WINDOW: u64 = 183_000;
/// The callbacks Claude calls before it compacts and as it starts again after.
pub const KEEP_HOOK: &str = "sikemux-keep";
pub const AFTER_COMPACT_HOOK: &str = "sikemux-after-compact";
/// The callback Claude calls before each tool, so a handoff note runs none, even one the
/// person's settings allow without asking.
pub const TOOL_HOOK: &str = "sikemux-tool";

/// What the core asks for when it compacts an idle chat itself.
pub const IDLE_FOCUS: &str = "Keep what the person asked for and decided, and the work still open.";

/// The hooks a Claude chat registers as it starts. Every chat is handed Sikemux's words back
/// after a compaction; only a capped one is told what to keep before it, and only one that
/// `rotates` has its tools refused while it writes a handoff note.
pub fn claude_hooks(capped: bool, rotates: bool) -> Value {
    let mut hooks = json!({
        "SessionStart": [{ "matcher": "compact", "hookCallbackIds": [AFTER_COMPACT_HOOK] }],
    });
    if capped {
        hooks["PreCompact"] = json!([{ "hookCallbackIds": [KEEP_HOOK] }]);
    }
    if rotates {
        hooks["PreToolUse"] = json!([{ "hookCallbackIds": [TOOL_HOOK] }]);
    }
    hooks
}
