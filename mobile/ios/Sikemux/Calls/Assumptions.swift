import Foundation
import Observation
import SikemuxCore

/// The questions an agent went on without, on an assumption it stated, read with the host's `questions` call whenever
/// the chat moves on, as the Mac reads them. Fine lets an assumption stand; Change answers it from the composer.
@MainActor @Observable
final class Assumptions {
    private static var kept: [String: Assumptions] = [:]

    static func of(_ core: String, _ agentId: String) -> Assumptions {
        let key = "\(core)/\(agentId)"
        if let found = kept[key] { return found }
        let made = Assumptions(core: core, agentId: agentId)
        kept[key] = made
        return made
    }

    let core: String
    let agentId: String
    private(set) var assumed: [AgentQuestion] = []
    /// The assumption whose answer the composer writes, after Change.
    private(set) var changing: AgentQuestion?
    private var sent: Set<String> = []
    @ObservationIgnored private var seen: String?
    @ObservationIgnored private var reading: Task<Void, Never>?

    private init(core: String, agentId: String) {
        self.core = core
        self.agentId = agentId
    }

    /// Reads the questions again when `seen` says the chat moved on.
    func read(_ seen: String, report: @escaping (String) -> Void) {
        guard seen != self.seen else { return }
        self.seen = seen
        reload(report)
    }

    private func reload(_ report: @escaping (String) -> Void) {
        reading?.cancel()
        reading = Task {
            guard let connection = Hub.shared.live(core).connection else { return }
            do {
                let found = try await connection.call(PhoneCall.Questions.self, NoParams())
                guard !Task.isCancelled else { return }
                assumed = found.filter { $0.agentId == agentId && $0.state == .assumed && !sent.contains($0.id) }
                if let changing, !assumed.contains(where: { $0.id == changing.id }) { self.changing = nil }
            } catch {
                if !Task.isCancelled { report(CallWords.couldNotRead(problem(error))) }
            }
        }
    }

    func change(_ question: AgentQuestion) {
        changing = question
    }

    func stopChanging() {
        changing = nil
    }

    /// Tells the agent its assumption holds.
    func fine(_ question: AgentQuestion, report: @escaping (String) -> Void) {
        Task { _ = await send(question, AnswerParams(id: question.id, fine: true), report: report) }
    }

    /// Answers an assumption with what the person wrote in the composer, with the files they sent.
    func answer(_ question: AgentQuestion, written: WrittenAnswer, report: @escaping (String) -> Void) async -> Bool {
        let params = AnswerParams(
            id: question.id, answer: written.answer, paths: written.paths, context: written.context)
        return await send(question, params, report: report)
    }

    private func send(_ question: AgentQuestion, _ params: AnswerParams, report: @escaping (String) -> Void) async
        -> Bool
    {
        guard let connection = Hub.shared.live(core).connection else { return false }
        sent.insert(question.id)
        assumed.removeAll { $0.id == question.id }
        if changing?.id == question.id { changing = nil }
        do {
            _ = try await connection.call(PhoneCall.AnswerQuestion.self, params)
            Haptics.success()
            return true
        } catch {
            sent.remove(question.id)
            Haptics.failure()
            report(problem(error))
            reload(report)
            return false
        }
    }
}
