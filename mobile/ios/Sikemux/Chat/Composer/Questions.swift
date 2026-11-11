import Foundation
import Observation
import SikemuxCore

/// The questions an agent put to the person, read with the host's `questions` call whenever the chat moves on.
/// Answers are held until every open question has one, then go together with `answerQuestions`; a lone question that
/// takes one answer goes on the tap. The first answer anywhere wins.
@MainActor @Observable
final class ChatQuestions {
    /// What an answer gives: written words, one choice, several, or that the assumption stands.
    enum Given {
        case written(WrittenAnswer)
        case choice(Int)
        case choices([Int])
        case fine

        func params(_ id: String) -> AnswerParams {
            switch self {
            case .written(let written):
                AnswerParams(id: id, answer: written.answer, paths: written.paths, context: written.context)
            case .choice(let choice): AnswerParams(id: id, choice: choice)
            case .choices(let choices): AnswerParams(id: id, choices: choices)
            case .fine: AnswerParams(id: id, fine: true)
            }
        }
    }

    /// An answer held for an open question until every open one has its own, and what shows was picked; empty for a
    /// secret.
    struct Pick {
        var given: Given
        var shown: String
    }

    private static var kept: [String: ChatQuestions] = [:]

    static func of(_ core: String, _ agentId: String) -> ChatQuestions {
        let key = "\(core)\n\(agentId)"
        if let found = kept[key] { return found }
        let made = ChatQuestions(core: core, agentId: agentId)
        kept[key] = made
        return made
    }

    let core: String
    let agentId: String
    private var all: [AgentQuestion] = []
    /// Answered from here, kept out of sight until the host's next list moves them on.
    private var sent: Set<String> = []
    private(set) var picks: [String: Pick] = [:]
    private var shownId: String?
    private var foldedId: String?
    private var writingId: String?
    private var dismissed: Set<String> = []
    @ObservationIgnored private var seen: String?
    @ObservationIgnored private var reading: Task<Void, Never>?
    @ObservationIgnored var report: (String) -> Void = { _ in }

    private init(core: String, agentId: String) {
        self.core = core
        self.agentId = agentId
    }

    private var own: [AgentQuestion] { all.filter { $0.agentId == agentId } }
    private func waiting(_ state: QuestionState) -> [AgentQuestion] {
        own.filter { $0.state == state && !sent.contains($0.id) }
    }

    /// What the agent asked the person and waits on, oldest first.
    var open: [AgentQuestion] { waiting(.open) }
    /// Questions the agent went on without, on an assumption.
    var assumed: [AgentQuestion] { waiting(.assumed) }
    /// Answered, until the agent has read the answer.
    var answered: [AgentQuestion] { own.filter { $0.state == .answered } }

    /// The open question on show: the one the person chose, else the first still without an answer.
    var current: AgentQuestion? {
        let open = open
        return open.first { $0.id == shownId } ?? open.first { picks[$0.id] == nil } ?? open.first
    }

    /// Every open question has an answer, so they can go together.
    var ready: Bool {
        let open = open
        return !open.isEmpty && open.allSatisfy { picks[$0.id] != nil }
    }

    var folded: Bool { foldedId != nil && open.first?.id == foldedId }

    private var writingQuestion: AgentQuestion? {
        (open + assumed).first { $0.id == writingId && !$0.secret }
    }

    /// The person chose to write the answer.
    var writing: Bool { writingQuestion != nil }

    /// The question whatever the composer sends answers, while one is open and the person has not taken the chip off;
    /// never a secret one, which has its own masked field.
    var composing: AgentQuestion? {
        if let writing = writingQuestion { return writing }
        guard let current, !current.secret, !folded, !dismissed.contains(current.id) else { return nil }
        return current
    }

    /// Reads the questions again when `seen` says the chat moved on.
    func read(_ seen: String) {
        guard seen != self.seen else { return }
        self.seen = seen
        reload()
    }

    private func reload() {
        reading?.cancel()
        reading = Task {
            guard let connection = Hub.shared.live(core).connection else { return }
            do {
                let found = try await connection.call(PhoneCall.Questions.self, NoParams())
                if !Task.isCancelled { all = found }
            } catch {
                if !Task.isCancelled { report("could not read this agent's questions: \(problem(error))") }
            }
        }
    }

    @discardableResult
    private func call(_ ids: [String], undo: (() -> Void)? = nil, _ send: @escaping (Connection) async throws -> Void)
        -> Bool
    {
        guard let connection = Hub.shared.live(core).connection else { return false }
        sent.formUnion(ids)
        if let writing = writingId, ids.contains(writing) { writingId = nil }
        Task {
            do {
                try await send(connection)
                picks = picks.filter { !ids.contains($0.key) }
                if let shown = shownId, ids.contains(shown) { shownId = nil }
            } catch {
                sent.subtract(ids)
                undo?()
                report(problem(error))
            }
            reload()
        }
        return true
    }

    /// Answers one question at once, which shows answered straight away.
    @discardableResult
    func answer(_ question: AgentQuestion, _ given: Given, undo: (() -> Void)? = nil) -> Bool {
        call([question.id], undo: undo) { connection in
            _ = try await connection.call(PhoneCall.AnswerQuestion.self, given.params(question.id))
        }
    }

    private func send(_ held: [String: Pick]) {
        let open = open
        guard !open.isEmpty, open.allSatisfy({ held[$0.id] != nil }) else { return }
        let answers = open.map { held[$0.id]!.given.params($0.id) }
        call(open.map(\.id)) { connection in
            _ = try await connection.call(PhoneCall.AnswerQuestions.self, AnswersParams(answers: answers))
        }
    }

    /// Holds `pick` and moves on to the next question still without an answer, or sends a lone question.
    private func pick(_ question: AgentQuestion, _ given: Pick) {
        var held = picks
        held[question.id] = given
        picks = held
        writingId = nil
        let open = open
        if open.count == 1 { return send(held) }
        let at = open.firstIndex { $0.id == question.id } ?? 0
        if let next = (Array(open[(at + 1)...]) + open).first(where: { held[$0.id] == nil }) { shownId = next.id }
    }

    /// Picks a choice, or on a question that takes several, ticks or unticks it.
    func choose(_ question: AgentQuestion, _ choice: Int) {
        let label = { (index: Int) in index < question.choices.count ? question.choices[index].label : "" }
        guard question.multi else { return pick(question, Pick(given: .choice(choice), shown: label(choice))) }
        var ticked: [Int] = []
        if case .choices(let was) = picks[question.id]?.given { ticked = was }
        let choices = ticked.contains(choice) ? ticked.filter { $0 != choice } : (ticked + [choice]).sorted()
        if choices.isEmpty {
            picks[question.id] = nil
        } else {
            picks[question.id] = Pick(given: .choices(choices), shown: choices.map(label).joined(separator: ", "))
        }
    }

    /// Holds a written answer for an open question, or answers an assumed one at once. `undo` puts it back where it
    /// was written when the host did not take it.
    func hold(_ question: AgentQuestion, _ written: WrittenAnswer, undo: (() -> Void)? = nil) -> Bool {
        if question.state == .assumed { return answer(question, .written(written), undo: undo) }
        let words = written.answer.isEmpty ? written.paths.joined(separator: ", ") : written.answer
        pick(question, Pick(given: .written(written), shown: question.secret ? "" : words))
        return true
    }

    func sendAll() { send(picks) }
    func show(_ question: AgentQuestion) { shownId = question.id }
    func fold(_ fold: Bool) { foldedId = fold ? open.first?.id : nil }

    func write(_ question: AgentQuestion) {
        writingId = question.id
        dismissed.remove(question.id)
    }

    func stopComposing() {
        guard let composing else { return }
        writingId = nil
        dismissed.insert(composing.id)
    }
}
