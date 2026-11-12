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
        if now.isEmpty { return text.map { AnswerCallParams(id: call.id, words: $0) } }
        if call.multi == true { return AnswerCallParams(id: call.id, options: now, note: text) }
        return AnswerCallParams(id: call.id, option: now[0], note: text)
    }
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
    private var drafts: [String: CallDraft] = [:]
    /// Calls whose chip the person took off the composer, so what it sends goes to the agent instead.
    private var composingOff: Set<String> = []
    @ObservationIgnored private var seen: Set<String> = []

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

    /// The oldest call agent `agentId` waits on the person for, which its chat docks.
    func waiting(for agentId: String) -> PersonCall? {
        (view?.calls ?? [])
            .filter { call in
                call.state == .open
                    && (call.asker.agentId == agentId || (call.joined ?? []).contains { $0.agentId == agentId })
            }
            .min { $0.raisedAt < $1.raisedAt }
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

    func draft(_ call: PersonCall) -> CallDraft {
        if let held = drafts[call.id] { return held }
        let made = CallDraft()
        drafts[call.id] = made
        return made
    }

    private static func settledAt(_ call: PersonCall) -> Int {
        call.outcome?.at ?? call.answer?.at ?? call.raisedAt
    }

    /// Tells the host once that this phone drew the call, which a production call needs before its answer counts.
    func saw(_ call: PersonCall) {
        guard seen.insert(call.id).inserted, let connection = Hub.shared.live(core).connection else { return }
        Task { _ = try? await connection.call(PhoneCall.CallSeen.self, CallParams(id: call.id)) }
    }

    /// Answers a call with what its card holds. The first answer anywhere wins.
    func answer(_ call: PersonCall, _ params: AnswerCallParams, report: @escaping (String) -> Void) {
        let draft = draft(call)
        guard !draft.sending, let connection = Hub.shared.live(core).connection else { return }
        draft.sending = true
        Task {
            defer { draft.sending = false }
            do {
                _ = try await connection.call(PhoneCall.AnswerCall.self, params)
                Haptics.success()
            } catch {
                Haptics.failure()
                report(problem(error))
            }
        }
    }

    /// Answers the call waiting in a chat with what the person typed in its composer, with the files they sent.
    func answer(_ call: PersonCall, written: WrittenAnswer) async -> Bool {
        guard let connection = Hub.shared.live(core).connection else { return false }
        do {
            // A call that takes typed words always comes from a question, which answerQuestion answers with files.
            if written.paths.isEmpty {
                _ = try await connection.call(
                    PhoneCall.AnswerCall.self, AnswerCallParams(id: call.id, words: written.answer))
            } else {
                _ = try await connection.call(
                    PhoneCall.AnswerQuestion.self,
                    AnswerParams(id: call.id, answer: written.answer, paths: written.paths, context: written.context))
            }
            return true
        } catch {
            Haptics.failure()
            return false
        }
    }

    /// Hands a call back to its asker's lead, or its asker, to decide.
    func notMine(_ call: PersonCall, report: @escaping (String) -> Void) {
        guard let connection = Hub.shared.live(core).connection else { return }
        Task {
            do {
                _ = try await connection.call(PhoneCall.CallNotMine.self, CallParams(id: call.id))
            } catch {
                report(problem(error))
            }
        }
    }
}
