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
        let date = Format.date(millis: at)
        let calendar = Calendar.current
        let clock = time.string(from: date)
        let days =
            calendar.dateComponents([.day], from: calendar.startOfDay(for: date), to: calendar.startOfDay(for: now)).day
            ?? 0
        if days == 0 { return "Today, \(clock)" }
        if days == 1 { return "Yesterday, \(clock)" }
        let sameYear = calendar.component(.year, from: date) == calendar.component(.year, from: now)
        return "\((sameYear ? day : dayAndYear).string(from: date)), \(clock)"
    }

    /// When a message came, as the line over someone else's bubble says it: today only the time.
    static func clock(_ at: Int, now: Date = Date()) -> String {
        let said = sent(at, now: now)
        return said.hasPrefix("Today, ") ? String(said.dropFirst("Today, ".count)) : said
    }

    static let earlierLost = "Earlier messages are no longer kept."

    /// What the line under the last row says the chat is doing, from what the core says it is doing.
    static func doing(_ activity: ChatActivity?) -> Doing? {
        switch activity {
        case .retrying(let retrying):
            let verb =
                retrying.connection
                ? "Reconnecting" : retrying.attempt.map { "Retrying, attempt \($0)" } ?? "Retrying"
            return Doing(verb, retrying.reason)
        case .call(let call): return doing(call.row)
        case .thinking: return Doing("Thinking")
        case .starting: return Doing("Starting agent")
        case .connecting: return Doing("Connecting to agent session")
        default: return nil
        }
    }

    private static let actionWords: [ToolAction: String] = [
        .read: "read", .edit: "edit", .delete: "delete", .move: "move", .search: "search", .execute: "run",
        .think: "think", .fetch: "fetch", .switchMode: "mode", .mcp: "mcp", .other: "tool",
    ]

    private static let actionDoing: [ToolAction: String] = [
        .read: "Reading", .edit: "Editing", .delete: "Deleting", .move: "Moving", .search: "Searching",
        .execute: "Running a command", .think: "Thinking", .fetch: "Fetching", .switchMode: "Switching mode",
        .mcp: "Working", .other: "Working",
    ]

    /// What a running call is doing: its verb, and what it is on in its row's words.
    private static func doing(_ row: ToolRow) -> Doing {
        let target = row.target
        let subject = row.subject ?? target
        let agent = { (name: String) in name.isEmpty || name.hasPrefix("@") ? name : "@\(name)" }
        let family = row.family
        switch row.intent {
        case .run, .test, .build, .git, .commit:
            if family == .shell { return Doing("Running", target, typed: true) }
            if row.intent == .commit { return Doing("Committing", target) }
        case .wait:
            return family == .waiting && row.subject != nil
                ? Doing("Waiting for", agent(subject)) : Doing("Waiting", target)
        case .message: return Doing("Messaging", agent(subject))
        case .start: return family == .agents ? Doing("Starting", agent(subject)) : Doing("Starting", subject)
        case .read:
            if family == .files || family == .shell { return Doing("Reading", subject) }
        case .view: return Doing("Reading", subject)
        case .search:
            return family == .web
                ? Doing("Searching the web for", subject) : Doing("Searching for", subject, typed: true)
        case .list: return Doing("Listing", subject)
        case .edit: return Doing("Editing", target)
        case .write: return Doing("Writing", target)
        case .open: return Doing("Opening", target)
        case .click: return Doing(family == .simulator ? "Tapping" : "Clicking", target)
        case .type: return Doing("Typing", target)
        case .look: return Doing("Taking a screenshot")
        case .fetch: return Doing("Reading", target)
        default: break
        }
        return Doing(row.activity ?? actionDoing[row.action] ?? "Working")
    }

    static func toolKind(_ tool: ToolCall) -> String {
        tool.row.verb ?? tool.row.server ?? actionWords[tool.row.action] ?? "tool"
    }
}

/// What the agent is doing, as a verb and what it is on: the command it runs, the agent it waits for, why it retries.
struct Doing: Equatable {
    let verb: String
    let what: String?
    /// `what` is a command, set as typed.
    let typed: Bool

    init(_ verb: String, _ what: String? = nil, typed: Bool = false) {
        let said = what?.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty == false
        self.verb = verb
        self.what = said ? what : nil
        self.typed = said && typed
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
