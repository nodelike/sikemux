import Foundation

/// The words the transcript says times, counts and calls in, as the Mac says them.
enum ChatWords {
    static func duration(ms: Double) -> String {
        let elapsed = max(0, ms)
        // Tool calls are often quicker than a tenth of a second, and rounding those to seconds reported every one of
        // them as the same 0.0s.
        if elapsed < 1000 { return "\(Int(elapsed.rounded()))ms" }
        return Self.elapsed(seconds: Int((elapsed / 1000).rounded()))
    }

    static func elapsed(seconds: Int) -> String {
        if seconds < 60 { return "\(seconds)s" }
        if seconds < 3600 { return "\(seconds / 60)m \(String(format: "%02d", seconds % 60))s" }
        return "\(seconds / 3600)h \(String(format: "%02d", (seconds / 60) % 60))m"
    }

    static func calls(_ count: Int) -> String { "\(count) tool \(count == 1 ? "call" : "calls")" }
    static func callCount(_ count: Int) -> String { "\(count) \(count == 1 ? "call" : "calls")" }

    /// A folded turn's line: whether it worked or only thought, and for how long.
    static func worked(_ took: Int?, thought: Bool) -> String {
        let verb = thought ? "Thought" : "Worked"
        guard let took else { return verb }
        return "\(verb) for \(duration(ms: Double(took)))"
    }

    static func rate(_ rate: Double) -> String {
        rate < 10 ? "~\(String(format: "%.1f", rate)) tok/s" : "~\(Int(rate.rounded())) tok/s"
    }

    private static let time: DateFormatter = {
        let formatter = DateFormatter()
        formatter.setLocalizedDateFormatFromTemplate("HH:mm")
        return formatter
    }()

    private static let day: DateFormatter = {
        let formatter = DateFormatter()
        formatter.setLocalizedDateFormatFromTemplate("d MMM")
        return formatter
    }()

    private static let dayAndYear: DateFormatter = {
        let formatter = DateFormatter()
        formatter.setLocalizedDateFormatFromTemplate("d MMM yyyy")
        return formatter
    }()

    /// When a message was sent, as a reader scanning the transcript wants it: "Today, 21:07", "Yesterday, 14:02",
    /// "29 Sep, 09:15".
    static func sent(_ at: Int, now: Date = Date()) -> String {
        let date = Date(timeIntervalSince1970: Double(at) / 1000)
        let calendar = Calendar.current
        let clock = time.string(from: date)
        let days = calendar.dateComponents([.day], from: calendar.startOfDay(for: date), to: calendar.startOfDay(for: now)).day ?? 0
        if days == 0 { return "Today, \(clock)" }
        if days == 1 { return "Yesterday, \(clock)" }
        let sameYear = calendar.component(.year, from: date) == calendar.component(.year, from: now)
        return "\((sameYear ? day : dayAndYear).string(from: date)), \(clock)"
    }

    static let earlierLost = "Earlier messages are no longer kept."

    /// What the line under the last row says the chat is doing, from what the core says it is doing.
    static func activity(_ activity: ChatActivity?) -> String? {
        switch activity {
        case .retrying(let retrying):
            return (retrying.connection ? "Reconnecting…" : "Retrying…") + (retrying.attempt.map { " (attempt \($0))" } ?? "")
        case .call(let call): return activityLabel(call.row)
        case .thinking: return "Thinking…"
        case .starting: return "Starting agent…"
        case .connecting: return "Connecting to agent session…"
        default: return nil
        }
    }

    private static let actionWords: [ToolAction: String] = [
        .read: "read", .edit: "edit", .delete: "delete", .move: "move", .search: "search", .execute: "run",
        .think: "think", .fetch: "fetch", .switchMode: "mode", .mcp: "mcp", .other: "tool",
    ]

    private static let actionSentences: [ToolAction: String] = [
        .read: "Reading…", .edit: "Editing…", .delete: "Deleting…", .move: "Moving…", .search: "Searching…",
        .execute: "Running a command…", .think: "Thinking…", .fetch: "Fetching…", .switchMode: "Switching mode…",
    ]

    /// The running row is one line, so it says what the agent is doing rather than quote the call back.
    static func activityLabel(_ row: ToolRow) -> String {
        row.activity ?? actionSentences[row.action] ?? "Working…"
    }

    static func toolKind(_ tool: ToolCall) -> String {
        tool.row.verb ?? tool.row.server ?? actionWords[tool.row.action] ?? "tool"
    }
}

extension ToolCall {
    var running: Bool {
        switch status ?? .pending {
        case .completed, .failed, .cancelled: false
        default: true
        }
    }
}
