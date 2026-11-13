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
            return Said(
                word: String(localized: "Working"), phase: .working,
                parts: [lowered(on), elapsed((nowMs - turn.since) / 1000)])
        case .waiting:
            guard let wait = turn.wait else { return nil }
            return Said(word: waitingOn(wait), phase: .waiting, parts: waited(wait, nowMs: nowMs).map { [$0] } ?? [])
        case .needsYou:
            guard let call = turn.call else { return nil }
            let more = call.count > 1 ? String(localized: " and \(call.count - 1) more") : ""
            return Said(
                word: String(localized: "Needs you"), phase: .needsYou,
                parts: ["\(call.question)\(more)", String(localized: "\(minutes(call.waitingSince)) min")])
        case .done:
            guard let done = turn.done else { return Said(word: String(localized: "Done"), phase: .done, parts: []) }
            var parts = [changed(done.changes)]
            if done.changes != .none {
                switch (done.checks, done.tests) {
                case (.pass, let tests?): parts.append(String(localized: "\(tests) tests pass"))
                case (.pass, nil): parts.append(String(localized: "checks pass"))
                case (.failed, _): parts.append(String(localized: "checks fail"))
                default: break
                }
            }
            return Said(word: String(localized: "Done"), phase: .done, parts: parts)
        case .stopped:
            var parts = turn.stopped?.left.map { [lowered($0)] } ?? []
            parts.append(String(localized: "nothing set to wake it"))
            return Said(word: String(localized: "Stopped"), phase: .stopped, parts: parts)
        case .failed:
            guard let failed = turn.failed else { return nil }
            let why: String =
                switch failed.why {
                case .limit: String(localized: "\(maker)'s limit")
                case .signIn: String(localized: "signed out of \(maker)")
                case .network: String(localized: "lost the connection to \(maker)")
                default: failed.said ?? String(localized: "the turn did not finish")
                }
            var parts = [why]
            if let resets = failed.resetsAt, resets > nowMs {
                parts.append(String(localized: "resets \(ChatWords.clock(resets))"))
            }
            return Said(word: String(localized: "Failed"), phase: .failed, parts: parts)
        case .unknown:
            return nil
        }
    }

    /// The line over a call docked on the chat, which already shows the question: the word, how many calls wait when
    /// more than one does, and how long.
    static func docked(_ said: Said, waiting: Int) -> Said {
        guard said.phase == .needsYou else { return said }
        let more = waiting > 1 ? [String(localized: "\(waiting) calls")] : []
        return Said(word: said.word, phase: said.phase, parts: more + Array(said.parts.suffix(1)))
    }

    static func waitingOn(_ wait: TurnWait) -> String {
        switch wait.kind {
        case .ci: String(localized: "Waiting on CI")
        case .deploy: String(localized: "Waiting on the deploy")
        case .check: String(localized: "Waiting on the checks")
        case .agent:
            wait.what.hasPrefix("@")
                ? String(localized: "Waiting on \(wait.what)") : String(localized: "Waiting on @\(wait.what)")
        case .timer:
            wait.until.map { String(localized: "Waiting until \(ChatWords.clock($0))") } ?? String(localized: "Waiting")
        case .process, .unknown: String(localized: "Waiting on \(wait.what)")
        }
    }

    /// How long a wait has gone, against how long it usually takes once that is known.
    static func waited(_ wait: TurnWait, nowMs: Int) -> String? {
        guard wait.kind != .timer else { return nil }
        let gone = max(0, nowMs - wait.startedAt) / 60_000
        guard let expected = wait.expectedMs else { return String(localized: "\(gone) min") }
        let usual = (expected + 59_999) / 60_000
        return String(localized: "\(gone) of ~\(usual) min")
    }

    static func changed(_ changes: WorkAt) -> String {
        switch changes {
        case .none, .unknown: String(localized: "no changes")
        case .uncommitted: String(localized: "uncommitted")
        case .committed: String(localized: "committed")
        case .landed: String(localized: "landed")
        case .staged: String(localized: "on staging")
        case .shipped: String(localized: "shipped")
        }
    }

    /// The words on a step's button.
    static func step(_ step: TurnStep) -> String? {
        switch step {
        case .commit: String(localized: "Commit")
        case .push: String(localized: "Push")
        case .deployStaging: String(localized: "Deploy to staging")
        case .shipProduction: String(localized: "Ship to production…")
        case .rollBack: String(localized: "Roll back")
        case .tryIt: String(localized: "Try it")
        case .carryOn: String(localized: "Carry on")
        case .undo: String(localized: "Undo")
        case .unknown: nil
        }
    }

    /// The button that brings the call a waiting agent asks into view.
    static let look = String(localized: "Look")

    /// How long a turn has run, clock style as the Mac says it: `45s`, `3m 07s`, `2h 05m`.
    static func elapsed(_ seconds: Int) -> String {
        let seconds = max(0, seconds)
        let two = { (value: Int) in value < 10 ? "0\(value)" : "\(value)" }
        if seconds < 60 { return String(localized: "\(seconds)s") }
        if seconds < 3600 { return String(localized: "\(seconds / 60)m \(two(seconds % 60))s") }
        return String(localized: "\(seconds / 3600)h \(two(seconds / 60 % 60))m")
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
            return turn.call.map { String(localized: "\(max(1, (nowMs - $0.waitingSince) / 60_000))m") }
        case .waiting:
            guard let wait = turn.wait else { return nil }
            if ringShare(wait, now: now) != nil, let expected = wait.expectedMs {
                let left = max(1, (wait.startedAt + expected - nowMs + 59_999) / 60_000)
                return String(localized: "\(left)m")
            }
            return wait.until.map { countdown($0 - nowMs) }
        default:
            return nil
        }
    }

    /// Time left until something fires: now, 12m, 3h, 4d; minutes rounded up under an hour.
    static func countdown(_ ms: Int) -> String {
        guard ms > 0 else { return String(localized: "now") }
        let minutes = (ms + 59_999) / 60_000
        if minutes < 60 { return String(localized: "\(minutes)m") }
        if minutes < 24 * 60 { return String(localized: "\(minutes / 60)h") }
        return String(localized: "\(minutes / (24 * 60))d")
    }

    /// What a row's mark says to VoiceOver, the hover card's words on the Mac.
    static func rowLabel(_ turn: TurnState, now: Date = Date()) -> String {
        switch turn.state {
        case .working: String(localized: "Working")
        case .waiting:
            turn.wait.map { "\(waitingOn($0)), \(rowMinutes(turn, now: now) ?? "")" } ?? String(localized: "Waiting")
        case .needsYou: String(localized: "Needs you, \(rowMinutes(turn, now: now) ?? "")")
        case .done: String(localized: "Done")
        case .stopped: String(localized: "Stopped")
        case .failed: String(localized: "Failed")
        case .unknown: ""
        }
    }

    // MARK: The call card

    static let pick = String(localized: "pick")
    static let againstIt = String(localized: "Against it:")
    static let addNote = String(localized: "Add a note…")
    static let ownWords = String(localized: "Or answer in your own words…")
    static let secretField = String(localized: "Type or paste it…")
    static let goWithThis = String(localized: "Go with this")
    static let notMine = String(localized: "Not mine")
    static let whatRuns = String(localized: "What runs")
    /// What the asker goes on with until the person answers, above Go with this.
    static func goesOn(_ name: String, _ assumption: String) -> String {
        String(localized: "Until you answer, @\(name) goes on assuming \(assumption)")
    }
    static func kept(_ name: String) -> String {
        String(localized: "Kept private: @\(name) reads it, and it never shows in the chat.")
    }
    /// Go with this while the host has not yet taken that this phone drew the call.
    static let connecting = String(localized: "Connecting…")
    /// Which of an agent's waiting calls the card shows: `1 of 3`.
    static func place(_ index: Int, of count: Int) -> String { String(localized: "\(index) of \(count)") }
    /// On the card when the answer sent from its notification did not go.
    static func pushFailed(_ why: String) -> String {
        String(localized: "Your answer from the notification didn't go: \(why)")
    }
    /// How much a hidden answer holds, so the person can see their typing and paste landed.
    static func hidden(_ text: String) -> String {
        let lines = (text.hasSuffix("\n") ? String(text.dropLast()) : text).components(separatedBy: "\n").count
        let characters =
            text.count == 1
            ? String(localized: "1 character hidden") : String(localized: "\(text.count) characters hidden")
        return lines > 1 ? String(localized: "\(lines) lines, \(characters)") : characters
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
        String(
            localized:
                "A call's answer takes words and files, not chips. Take the chips off, or send them to the agent.")
    /// The composer's chip while a call waits in the chat.
    static func answering(_ name: String, _ question: String) -> String {
        String(localized: "Answering @\(name) · \(question)")
    }
    /// The composer's placeholder while a call waits in the chat.
    static func typeYourAnswer(_ name: String) -> String { String(localized: "Or type your answer to @\(name)") }

    /// A weighty call's brief, which stands as its context.
    static func context(_ call: PersonCall) -> String? {
        call.brief.map { "\($0.tried) \($0.shows) \($0.cannotUndo)" } ?? call.context
    }

    // MARK: The chief

    /// The chief's row: what follows it is its chat's own title, and where projects share the list, the project.
    static let chief = "@Chief"
    /// The composer's words in the chief's own chat.
    static let tellChief = String(localized: "Tell @Chief what you want")
    /// Under the chief's row when a tap cannot reach the host to open its chat.
    static let chiefOffline = String(localized: "Not connected to this host yet, so its chief can't open.")
    /// The composer's words in a worker's chat whose project's chief hears what is typed there.
    static func sendToTold(_ name: String) -> String { String(localized: "Send to @\(name) · @Chief is told") }

    // MARK: The chief's chat: what happened while the person was away, an Overrule, the week's count

    static let whileAway = String(localized: "While you were away")
    /// How long the person was away, `back - since`.
    static func awayFor(_ milliseconds: Int) -> String {
        let total = max(0, milliseconds) / 60_000
        let (hours, minutes) = (total / 60, total % 60)
        if hours == 0 { return String(localized: "\(minutes) min") }
        if minutes == 0 { return String(localized: "\(hours) h") }
        return String(localized: "\(hours) h \(minutes) min")
    }
    static let awayNeedsYou = String(localized: "Needs you")
    static let decidedForYou = String(localized: "Decided for you")
    static let landed = String(localized: "Landed")
    /// Under Needs you: the calls are the queue docked above the composer.
    static func answerBelow(_ count: Int) -> String {
        count == 1
            ? String(localized: "Answer it in the card below") : String(localized: "Answer them in the card below")
    }
    static let overrule = String(localized: "Overrule")
    /// What a tap on a call's cut context does, for VoiceOver.
    static let showAll = String(localized: "Shows all of it")
    static let showLess = String(localized: "Shows two lines")
    /// Under an Overrule or Undo that has no host to send it to.
    static let notConnected = String(localized: "Not connected to the host.")
    static let undo = String(localized: "Undo")
    static let overruled = String(localized: "Overruled")
    static let overruledUndone = String(localized: "Overruled, then undone")
    static let standsAgain = String(localized: "It stands again")
    static let stoppedAuto = String(localized: "This stopped Auto's run, so it can't be put back")
    static let fromNowOn = String(localized: "Decisions like this come to you from now on")
    /// A rule change's line as the Mac words it: its title, then the short hash of the check that holds it or who did
    /// it.
    static func ruleChange(_ line: RuleChange) -> (head: String, tail: String?) {
        if line.restored == true {
            return (String(localized: "Rule back"), line.backBy.map { String(localized: "by @\($0)") })
        }
        if let commit = line.commit { return (String(localized: "Now a check"), String(commit.prefix(7))) }
        let head = line.was != nil ? String(localized: "Rule changed") : String(localized: "Rule removed")
        return (head, String(localized: "by @\(line.by)"))
    }
    /// What was decided, quoted, never as the person's own words.
    static func quoted(_ text: String) -> String { String(localized: "“\(text)”") }
    /// When the week's count came: `Monday · 09:00`.
    static func weekCame(_ at: Date) -> String {
        let days = [
            String(localized: "Sunday"), String(localized: "Monday"), String(localized: "Tuesday"),
            String(localized: "Wednesday"), String(localized: "Thursday"), String(localized: "Friday"),
            String(localized: "Saturday"),
        ]
        let day = days[Calendar.current.component(.weekday, from: at) - 1]
        let time = at.formatted(date: .omitted, time: .shortened)
        return String(localized: "\(day) · \(time)")
    }
    /// The chief's count of a week, as the Mac words it.
    static func weekCount(_ week: ChiefWeek) -> String {
        let (decided, overruled) = (Int(week.decided), Int(week.overruled))
        let first: String =
            switch (decided, overruled) {
            case (1, 0): String(localized: "Last week I decided 1 thing, and you overruled none.")
            case (1, _): String(localized: "Last week I decided 1 thing, and you overruled \(overruled).")
            case (_, 0): String(localized: "Last week I decided \(decided) things, and you overruled none.")
            default: String(localized: "Last week I decided \(decided) things, and you overruled \(overruled).")
            }
        let (answered, picked) = (Int(week.answered), Int(week.asPicked))
        if answered == 0 { return first }
        let second =
            answered == 1
            ? String(localized: "You answered \(picked) of 1 call as I picked.")
            : String(localized: "You answered \(picked) of \(answered) calls as I picked.")
        return String(localized: "\(first) \(second)")
    }

    // MARK: An assumption

    /// After the agent's name: what it went on assuming.
    static func assumed(_ assumption: String) -> String { String(localized: "assumed \(assumption)") }
    static let fine = String(localized: "Fine")
    static let change = String(localized: "Change")
    static func couldNotRead(_ problem: String) -> String {
        String(localized: "could not read this agent's questions: \(problem)")
    }

    // MARK: The calls tab

    static let calls = String(localized: "Calls")
    static let answered = String(localized: "Answered")
    static let noCalls = String(localized: "Nothing waits on you.")

    /// What came of an answered or closed call, as its row in the calls tab says it.
    static func cameOf(_ call: PersonCall) -> String {
        switch call.outcome?.result {
        case .shipped:
            switch call.outcome?.healthy {
            case true?: return String(localized: "shipped · healthy")
            case false?: return String(localized: "shipped · unhealthy")
            case nil: return String(localized: "shipped")
            }
        case .notShipped: return String(localized: "not shipped")
        case .failed: return String(localized: "it failed")
        case .mayHaveRun: return String(localized: "may have run, Sikemux restarted")
        case .handedBack: return String(localized: "handed back")
        case .withdrawn: return String(localized: "went on without you")
        case .heard, .unknown, nil: break
        }
        guard let answer = call.answer else { return String(localized: "closed") }
        if call.secret == true { return String(localized: "answered, kept private") }
        if answer.words != nil { return String(localized: "in your words") }
        let picked = answer.option.map { [$0] } ?? answer.options ?? []
        if !picked.isEmpty, picked == call.pick.map({ [$0] }) { return String(localized: "as picked") }
        let labels = picked.compactMap { id in call.options.first { $0.id == id }?.label }
        return labels.isEmpty ? String(localized: "answered") : labels.map { "“\($0)”" }.joined(separator: ", ")
    }

    // The push's buttons say what `CallPush` in `Shared/Notify/Card.swift` holds, where the notification extension
    // reads them.
}
