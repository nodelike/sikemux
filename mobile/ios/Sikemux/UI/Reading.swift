import Observation
import SikemuxCore

/// A host call's answer as last read, or why reading it failed; what was read stays while a later read fails.
@MainActor @Observable
final class Reading<Answer> {
    var data: Answer?
    var problem: String?

    func read(_ live: HostLive, _ ask: (Connection) async throws -> Answer) async {
        guard let connection = live.connection else { return }
        do {
            data = try await ask(connection)
            problem = nil
        } catch {
            problem = Sikemux.problem(error)
        }
    }
}
