import Foundation
import Observation

/// What the person has done on a call's card while it waits: the options chosen, the note or their own words, and
/// whether what runs shows.
@MainActor @Observable
final class CallDraft {
    /// The options chosen; until the person touches the rows, the asker's pick.
    var chosen: [String]?
    var typed = ""
    var runsOpen = false
    var sending = false

    func chosen(in call: PersonCall) -> [String] {
        chosen ?? (call.pick.map { [$0] } ?? [])
    }

    /// Chooses an option, or clears the one chosen; on a call that takes several, ticks it.
    func choose(_ option: String, in call: PersonCall) {
        var now = chosen(in: call)
        if let at = now.firstIndex(of: option) {
            now.remove(at: at)
        } else if call.multi == true {
            now.append(option)
        } else {
            now = [option]
        }
        chosen = now
    }

    /// What answers the call as the card stands: the options chosen with the note, or the person's own words alone.
    func answer(_ call: PersonCall) -> AnswerCallParams? {
        let words = typed.trimmingCharacters(in: .whitespacesAndNewlines)
        let text = words.isEmpty ? nil : words
        let now = chosen(in: call)
        if now.isEmpty {
            if let text { return AnswerCallParams(id: call.id, words: text) }
            return call.goesOnAsIs ? AnswerCallParams(id: call.id, fine: true) : nil
        }
        if call.multi == true { return AnswerCallParams(id: call.id, options: now, note: text) }
        return AnswerCallParams(id: call.id, option: now[0], note: text)
    }
}

extension PersonCall {
    /// A call with nothing to pick that its asker goes on with: Go with this, with nothing typed, lets it stand.
    var goesOnAsIs: Bool { assumption != nil && options.isEmpty }
}

/// The calls one host puts to the person, as its view lists them, with what the person has done on each card, and
/// the calls that reach that host.
@MainActor @Observable
final class HostCalls {
    private static var kept: [String: HostCalls] = [:]

    static func of(_ core: String) -> HostCalls {
        if let found = kept[core] { return found }
        let made = HostCalls(core: core)
        kept[core] = made
        return made
    }

    let core: String
    /// What the person has done on each open card, kept out of observation: each draft is observed on its own.
    @ObservationIgnored private var drafts: [String: CallDraft] = [:]
    /// Calls whose chip the person took off the composer, so what it sends goes to the agent instead.
    private var composingOff: Set<String> = []
    /// The calls the core has taken word that this phone drew, which a production call needs before its answer counts.
    private(set) var seen: Set<String> = []
    @ObservationIgnored private var seeing: Set<String> = []
    /// The call a chat shows, by the chat's agent, when the person chose one other than the oldest.
    private var focused: [String: String] = [:]
    /// Why an answer from a notification did not go, by call.
    private(set) var failed: [String: String] = [:]

    private init(core: String) {
        self.core = core
    }

    private var view: DeviceView? { Hub.shared.live(core).view }

    /// The calls waiting on the person, newest first, in `project` or in every project.
    func open(in project: ProjectInfo?) -> [PersonCall] {
        (view?.calls ?? [])
            .filter { $0.state == .open && (project == nil || $0.project == project?.path) }
            .sorted { $0.raisedAt > $1.raisedAt }
    }

    /// The calls answered or closed lately, newest first, in `project` or in every project.
    func settled(in project: ProjectInfo?) -> [PersonCall] {
        (view?.calls ?? [])
            .filter { $0.state != .open && (project == nil || $0.project == project?.path) }
            .sorted { Self.settledAt($0) > Self.settledAt($1) }
    }

    /// Every call agent `agentId` waits on the person for, as asker or joined to it, oldest first.
    func queue(for agentId: String) -> [PersonCall] {
        (view?.calls ?? [])
            .filter { call in
                call.state == .open
                    && (call.asker.agentId == agentId || (call.joined ?? []).contains { $0.agentId == agentId })
            }
            .sorted { $0.raisedAt < $1.raisedAt }
    }

    /// The call agent `agentId`'s chat docks: the one the person chose, while it waits, else the oldest.
    func waiting(for agentId: String) -> PersonCall? {
        let queue = queue(for: agentId)
        return queue.first { $0.id == focused[agentId] } ?? queue.first
    }

    /// Makes `call` the one the chat of agent `chat` docks.
    func focus(_ call: PersonCall, in chat: String) {
        focused[chat] = call.id
    }

    /// The call whatever a chat's composer sends answers, while one waits there and the person has not taken its chip
    /// off. A step to production, or a secret, is answered only on its card.
    func composing(for agentId: String) -> PersonCall? {
        waiting(for: agentId).flatMap { call in
            composingOff.contains(call.id) || call.kind == .production || call.secret == true ? nil : call
        }
    }

    func stopComposing(_ call: PersonCall) {
        composingOff.insert(call.id)
    }

    /// What the person has done on `call`'s card. Drafts of calls no longer open go.
    func draft(_ call: PersonCall) -> CallDraft {
        if let held = drafts[call.id] { return held }
        let open = Set((view?.calls ?? []).filter { $0.state == .open }.map(\.id))
        drafts = drafts.filter { open.contains($0.key) }
        let made = CallDraft()
        drafts[call.id] = made
        return made
    }

    private static func settledAt(_ call: PersonCall) -> Int {
        call.outcome?.at ?? call.answer?.at ?? call.raisedAt
    }

    /// Tells the host this phone drew the call, until the core has taken it: a few tries a few seconds apart, and
    /// again when the card sees a new connection.
    func see(_ call: PersonCall) async {
        guard !seen.contains(call.id), !seeing.contains(call.id) else { return }
        seeing.insert(call.id)
        defer { seeing.remove(call.id) }
        for wait in [0, 2, 5, 10] {
            if wait > 0 { try? await Task.sleep(for: .seconds(wait)) }
            guard !Task.isCancelled, let connection = Hub.shared.live(core).connection else { return }
            do {
                _ = try await connection.call(PhoneCall.CallSeen.self, CallParams(id: call.id))
                seen.insert(call.id)
                return
            } catch {
                print("sikemux: the host did not take that this phone drew a call: \(error)")
            }
        }
    }

    /// Answers a call with what its card holds, once per tap: the card stays sending until the call leaves it. The
    /// first answer anywhere wins.
    func answer(_ call: PersonCall, _ params: AnswerCallParams, report: @escaping (String) -> Void) {
        let draft = draft(call)
        guard !draft.sending, let connection = Hub.shared.live(core).connection else { return }
        draft.sending = true
        failed[call.id] = nil
        Task {
            do {
                _ = try await connection.call(PhoneCall.AnswerCall.self, params)
                Haptics.success()
                if call.secret == true {
                    draft.typed = ""
                    drafts[call.id] = nil
                }
            } catch {
                draft.sending = false
                Haptics.failure()
                report(problem(error))
            }
        }
    }

    /// Answers the call waiting in a chat with what the person typed in its composer, with the files they sent.
    func answer(_ call: PersonCall, written: WrittenAnswer, report: @escaping (String) -> Void) async -> Bool {
        guard let connection = Hub.shared.live(core).connection else { return false }
        do {
            _ = try await connection.call(
                PhoneCall.AnswerCall.self,
                AnswerCallParams(id: call.id, words: written.answer, paths: written.paths.isEmpty ? nil : written.paths)
            )
            return true
        } catch {
            Haptics.failure()
            report(problem(error))
            return false
        }
    }

    /// Hands a call back to its asker's lead, or its asker, to decide, once per tap.
    func notMine(_ call: PersonCall, report: @escaping (String) -> Void) {
        let draft = draft(call)
        guard !draft.sending, let connection = Hub.shared.live(core).connection else { return }
        draft.sending = true
        Task {
            do {
                _ = try await connection.call(PhoneCall.CallNotMine.self, CallParams(id: call.id))
            } catch {
                draft.sending = false
                report(problem(error))
            }
        }
    }

    /// An answer from a notification did not go: its card says so, and why, until it is answered.
    func answerFailed(_ callId: String, _ why: String) {
        let live = Hub.shared.live(core)
        if live.connection != nil, let view = live.view {
            let open = Set((view.calls ?? []).filter { $0.state == .open }.map(\.id))
            guard open.contains(callId) else { return }
            failed = failed.filter { open.contains($0.key) }
        }
        failed[callId] = why
    }
}
