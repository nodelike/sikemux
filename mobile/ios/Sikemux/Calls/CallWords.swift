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

    /// The line over a call docked on the chat, which already shows the question: the word and how long it waited.
    static func docked(_ said: Said) -> Said {
        said.phase == .needsYou ? Said(word: said.word, phase: said.phase, parts: Array(said.parts.suffix(1))) : said
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

    /// How long a turn has run: `48s`, `1m 12s`, `1h 5m`.
    static func elapsed(_ seconds: Int) -> String {
        let seconds = max(0, seconds)
        if seconds < 60 { return "\(seconds)s" }
        if seconds < 3600 { return seconds % 60 == 0 ? "\(seconds / 60)m" : "\(seconds / 60)m \(seconds % 60)s" }
        return "\(seconds / 3600)h \(seconds % 3600 / 60)m"
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

    /// The minutes beside a row's mark: how long left on a wait whose length is known, else how long it has gone;
    /// how long a call has waited on the person.
    static func rowMinutes(_ turn: TurnState, now: Date = Date()) -> String? {
        let nowMs = Int(now.timeIntervalSince1970 * 1000)
        switch turn.state {
        case .waiting:
            guard let wait = turn.wait else { return nil }
            if let until = wait.until { return "\(max(1, (until - nowMs + 59_999) / 60_000))m" }
            if let expected = wait.expectedMs {
                return "\(max(1, (wait.startedAt + expected - nowMs + 59_999) / 60_000))m"
            }
            return "\(max(1, (nowMs - wait.startedAt) / 60_000))m"
        case .needsYou:
            return turn.call.map { "\(max(1, (nowMs - $0.waitingSince) / 60_000))m" }
        default:
            return nil
        }
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
    static let kept = "Kept private: it never shows in the chat."
    /// The composer's chip while a call waits in the chat.
    static func answering(_ name: String, _ question: String) -> String { "Answering @\(name) · \(question)" }
    /// The composer's placeholder while a call waits in the chat.
    static func typeYourAnswer(_ name: String) -> String { "Or type your answer to @\(name)" }

    /// A weighty call's brief, which stands as its context.
    static func context(_ call: PersonCall) -> String? {
        call.brief.map { "\($0.tried) \($0.shows) \($0.cannotUndo)" } ?? call.context
    }

    // MARK: The chief

    /// The chief's row, with its project beside it where projects share the list.
    static func chief(_ project: String?) -> String { project.map { "@Chief · \($0)" } ?? "@Chief" }
    /// Under the chief's row: its team.
    static func chiefTeam(_ agents: Int) -> String? {
        switch agents {
        case 0: nil
        case 1: "1 agent"
        default: "\(agents) agents"
        }
    }

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
