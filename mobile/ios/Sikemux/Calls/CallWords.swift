import Foundation

/// Every word the phone says about how a turn stands and about a call, in one table (the push's buttons aside), as the Mac's
/// `app/src/turn_line.rs` and `app/src/calls.rs` say them. The Android app copies this table.
enum CallWords {
    // MARK: The turn's line

    /// The state's word, then what it is on and how long, said between dots.
    struct Said: Equatable {
        let word: String
        let phase: TurnPhase
        let parts: [String]
    }

    /// What `turn` says for an agent of `maker` (its own name, such as Claude). Working with nothing said about what
    /// it is on says nothing, so the chat shows its live activity instead.
    static func said(_ turn: TurnState, maker: String, now: Date = Date()) -> Said? {
        let nowMs = Int(now.timeIntervalSince1970 * 1000)
        let minutes = { (from: Int) in max(1, (nowMs - from) / 60_000) }
        switch turn.state {
        case .working:
            guard let on = turn.on else { return nil }
            return Said(word: "Working", phase: .working, parts: [lowered(on), elapsed((nowMs - turn.since) / 1000)])
        case .waiting:
            guard let wait = turn.wait else { return nil }
            return Said(word: waitingOn(wait), phase: .waiting, parts: waited(wait, nowMs: nowMs).map { [$0] } ?? [])
        case .needsYou:
            guard let call = turn.call else { return nil }
            let more = call.count > 1 ? " and \(call.count - 1) more" : ""
            return Said(
                word: "Needs you", phase: .needsYou,
                parts: ["\(call.question)\(more)", "\(minutes(call.waitingSince)) min"])
        case .done:
            guard let done = turn.done else { return Said(word: "Done", phase: .done, parts: []) }
            var parts = [changed(done.changes)]
            if done.changes != .none {
                switch (done.checks, done.tests) {
                case (.pass, let tests?): parts.append("\(tests) tests pass")
                case (.pass, nil): parts.append("checks pass")
                case (.failed, _): parts.append("checks fail")
                default: break
                }
            }
            return Said(word: "Done", phase: .done, parts: parts)
        case .stopped:
            var parts = turn.stopped?.left.map { [lowered($0)] } ?? []
            parts.append("nothing set to wake it")
            return Said(word: "Stopped", phase: .stopped, parts: parts)
        case .failed:
            guard let failed = turn.failed else { return nil }
            let why: String =
                switch failed.why {
                case .limit: "\(maker)'s limit"
                case .signIn: "signed out of \(maker)"
                case .network: "lost the connection to \(maker)"
                default: failed.said ?? "the turn did not finish"
                }
            var parts = [why]
            if let resets = failed.resetsAt, resets > nowMs { parts.append("resets \(ChatWords.clock(resets))") }
            return Said(word: "Failed", phase: .failed, parts: parts)
        case .unknown:
            return nil
        }
    }

    /// The line over a call docked on the chat, which already shows the question: the word, how many calls wait when
    /// more than one does, and how long.
    static func docked(_ said: Said, waiting: Int) -> Said {
        guard said.phase == .needsYou else { return said }
        let more = waiting > 1 ? ["\(waiting) calls"] : []
        return Said(word: said.word, phase: said.phase, parts: more + Array(said.parts.suffix(1)))
    }

    static func waitingOn(_ wait: TurnWait) -> String {
        switch wait.kind {
        case .ci: "Waiting on CI"
        case .deploy: "Waiting on the deploy"
        case .check: "Waiting on the checks"
        case .agent: wait.what.hasPrefix("@") ? "Waiting on \(wait.what)" : "Waiting on @\(wait.what)"
        case .timer: wait.until.map { "Waiting until \(ChatWords.clock($0))" } ?? "Waiting"
        case .process, .unknown: "Waiting on \(wait.what)"
        }
    }

    /// How long a wait has gone, against how long it usually takes once that is known.
    static func waited(_ wait: TurnWait, nowMs: Int) -> String? {
        guard wait.kind != .timer else { return nil }
        let gone = max(0, nowMs - wait.startedAt) / 60_000
        guard let expected = wait.expectedMs else { return "\(gone) min" }
        return "\(gone) of ~\((expected + 59_999) / 60_000) min"
    }

    static func changed(_ changes: WorkAt) -> String {
        switch changes {
        case .none, .unknown: "no changes"
        case .uncommitted: "uncommitted"
        case .committed: "committed"
        case .landed: "landed"
        case .staged: "on staging"
        case .shipped: "shipped"
        }
    }

    /// The words on a step's button.
    static func step(_ step: TurnStep) -> String? {
        switch step {
        case .commit: "Commit"
        case .push: "Push"
        case .deployStaging: "Deploy to staging"
        case .shipProduction: "Ship to production…"
        case .rollBack: "Roll back"
        case .tryIt: "Try it"
        case .carryOn: "Carry on"
        case .undo: "Undo"
        case .unknown: nil
        }
    }

    /// The button that brings the call a waiting agent asks into view.
    static let look = "Look"

    /// How long a turn has run, clock style as the Mac says it: `45s`, `3m 07s`, `2h 05m`.
    static func elapsed(_ seconds: Int) -> String {
        let seconds = max(0, seconds)
        let two = { (value: Int) in value < 10 ? "0\(value)" : "\(value)" }
        if seconds < 60 { return "\(seconds)s" }
        if seconds < 3600 { return "\(seconds / 60)m \(two(seconds % 60))s" }
        return "\(seconds / 3600)h \(two(seconds / 60 % 60))m"
    }

    /// The agent's own words, as they read after a dot: the first letter small unless it starts a name.
    static func lowered(_ words: String) -> String {
        var trimmed = words.trimmingCharacters(in: .whitespaces)
        while trimmed.hasSuffix(".") { trimmed.removeLast() }
        guard let first = trimmed.first, trimmed.count > 1 else { return trimmed }
        let second = trimmed[trimmed.index(after: trimmed.startIndex)]
        return second.isUppercase ? trimmed : first.lowercased() + trimmed.dropFirst()
    }

    // MARK: A row's short mark

    /// How far a CI or check wait with an estimate has gone, which its row draws as a filling ring; nil for any other
    /// wait, which the row draws as the pink timer.
    static func ringShare(_ wait: TurnWait?, now: Date = Date()) -> Double? {
        guard let wait, wait.kind == .ci || wait.kind == .check, let expected = wait.expectedMs, expected > 0 else {
            return nil
        }
        let gone = now.timeIntervalSince1970 * 1000 - Double(wait.startedAt)
        return min(1, max(0, gone / Double(expected)))
    }

    /// The time beside a row's mark: how long a call has waited, the minutes left on a wait with an estimate, or how
    /// long until a set time; nothing else has a time.
    static func rowMinutes(_ turn: TurnState, now: Date = Date()) -> String? {
        let nowMs = Int(now.timeIntervalSince1970 * 1000)
        switch turn.state {
        case .needsYou:
            return turn.call.map { "\(max(1, (nowMs - $0.waitingSince) / 60_000))m" }
        case .waiting:
            guard let wait = turn.wait else { return nil }
            if ringShare(wait, now: now) != nil, let expected = wait.expectedMs {
                return "\(max(1, (wait.startedAt + expected - nowMs + 59_999) / 60_000))m"
            }
            return wait.until.map { countdown($0 - nowMs) }
        default:
            return nil
        }
    }

    /// Time left until something fires: now, 12m, 3h, 4d; minutes rounded up under an hour.
    static func countdown(_ ms: Int) -> String {
        guard ms > 0 else { return "now" }
        let minutes = (ms + 59_999) / 60_000
        if minutes < 60 { return "\(minutes)m" }
        if minutes < 24 * 60 { return "\(minutes / 60)h" }
        return "\(minutes / (24 * 60))d"
    }

    /// What a row's mark says to VoiceOver, the hover card's words on the Mac.
    static func rowLabel(_ turn: TurnState, now: Date = Date()) -> String {
        switch turn.state {
        case .working: "Working"
        case .waiting: turn.wait.map { "\(waitingOn($0)), \(rowMinutes(turn, now: now) ?? "")" } ?? "Waiting"
        case .needsYou: "Needs you, \(rowMinutes(turn, now: now) ?? "")"
        case .done: "Done"
        case .stopped: "Stopped"
        case .failed: "Failed"
        case .unknown: ""
        }
    }

    // MARK: The call card

    static let pick = "pick"
    static let againstIt = "Against it:"
    static let addNote = "Add a note…"
    static let ownWords = "Or answer in your own words…"
    static let secretField = "Type or paste it…"
    static let goWithThis = "Go with this"
    static let notMine = "Not mine"
    static let whatRuns = "What runs"
    static func kept(_ name: String) -> String { "Kept private: @\(name) reads it, and it never shows in the chat." }
    /// Go with this while the host has not yet taken that this phone drew the call.
    static let connecting = "Connecting…"
    /// Which of an agent's waiting calls the card shows: `1 of 3`.
    static func place(_ index: Int, of count: Int) -> String { "\(index) of \(count)" }
    /// On the card when the answer sent from its notification did not go.
    static func pushFailed(_ why: String) -> String { "Your answer from the notification didn't go: \(why)" }
    /// How much a hidden answer holds, so the person can see their typing and paste landed.
    static func hidden(_ text: String) -> String {
        let lines = (text.hasSuffix("\n") ? String(text.dropLast()) : text).components(separatedBy: "\n").count
        let characters = "\(text.count) character\(text.count == 1 ? "" : "s") hidden"
        return lines > 1 ? "\(lines) lines, \(characters)" : characters
    }
    /// The name on a picture's corner: an evidence item's own words, when they are short enough to be a name (Now,
    /// New), else none.
    static func pictureLabel(_ words: String) -> String? {
        let trimmed = words.trimmingCharacters(in: .whitespacesAndNewlines)
        let count = trimmed.split(whereSeparator: \.isWhitespace).count
        return !trimmed.isEmpty && trimmed.count <= 20 && count <= 3 ? trimmed : nil
    }
    /// Said when an answer to a call carries context chips, which a call's answer does not take.
    static let noChips =
        "A call's answer takes words and files, not chips. Take the chips off, or send them to the agent."
    /// The composer's chip while a call waits in the chat.
    static func answering(_ name: String, _ question: String) -> String { "Answering @\(name) · \(question)" }
    /// The composer's placeholder while a call waits in the chat.
    static func typeYourAnswer(_ name: String) -> String { "Or type your answer to @\(name)" }

    /// A weighty call's brief, which stands as its context.
    static func context(_ call: PersonCall) -> String? {
        call.brief.map { "\($0.tried) \($0.shows) \($0.cannotUndo)" } ?? call.context
    }

    // MARK: The chief

    /// The chief's row: what follows it is its chat's own title, and where projects share the list, the project.
    static let chief = "@Chief"
    /// The composer's words in the chief's own chat.
    static let tellChief = "Tell @Chief what you want"
    /// The composer's words in a worker's chat, in a project with a chief.
    static func sendTo(_ name: String) -> String { "Send to @\(name)" }
    /// Under the chief's row when a tap cannot reach the host to open its chat.
    static let chiefOffline = "Not connected to this host yet, so its chief can't open."

    // MARK: An assumption

    /// After the agent's name: what it went on assuming.
    static func assumed(_ assumption: String) -> String { "assumed \(assumption)" }
    static let fine = "Fine"
    static let change = "Change"
    static func couldNotRead(_ problem: String) -> String { "could not read this agent's questions: \(problem)" }

    // MARK: The calls tab

    static let calls = "Calls"
    static let answered = "Answered"
    static let noCalls = "Nothing waits on you."

    /// What came of an answered or closed call, as its row in the calls tab says it.
    static func cameOf(_ call: PersonCall) -> String {
        switch call.outcome?.result {
        case .shipped:
            switch call.outcome?.healthy {
            case true?: return "shipped · healthy"
            case false?: return "shipped · unhealthy"
            case nil: return "shipped"
            }
        case .notShipped: return "not shipped"
        case .failed: return "it failed"
        case .mayHaveRun: return "may have run, Sikemux restarted"
        case .handedBack: return "handed back"
        case .withdrawn: return "went on without you"
        case .heard, .unknown, nil: break
        }
        guard let answer = call.answer else { return "closed" }
        if call.secret == true { return "answered, kept private" }
        if answer.words != nil { return "in your words" }
        let picked = answer.option.map { [$0] } ?? answer.options ?? []
        if !picked.isEmpty, picked == call.pick.map({ [$0] }) { return "as picked" }
        let labels = picked.compactMap { id in call.options.first { $0.id == id }?.label }
        return labels.isEmpty ? "answered" : labels.map { "“\($0)”" }.joined(separator: ", ")
    }

    // The push's buttons say what `CallPush` in `Shared/Notify/Card.swift` holds, where the notification extension
    // reads them.
}
