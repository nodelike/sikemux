import OSLog

private let log = Logger(subsystem: "com.nodelike.sikemux", category: "on-screen")

/// Tells each host which of its chats this phone shows, as the Mac reports its screen: a chat shown stays awake, and one
/// shown with the app in front and the phone unlocked counts as read, and for the chief as the person being with it.
/// With the app behind or the phone locked, the host hears an empty list.
@MainActor
final class OnScreen {
    static let shared = OnScreen()
    private var shown: [String: String] = [:]
    private var sent: [String: OnScreenParams] = [:]
    private var inFront = true

    /// Chat `agentId` came on screen, on a phone with full access to the host.
    func show(_ core: String, _ agentId: String) {
        shown[core] = agentId
        tell(core)
    }

    func hide(_ core: String, _ agentId: String) {
        if shown[core] == agentId { shown[core] = nil }
        tell(core)
    }

    /// The app came to the front, or went behind or the phone locked.
    func front(_ active: Bool) {
        inFront = active
        for core in Set(shown.keys).union(sent.keys) { tell(core) }
    }

    /// Says it again once the host can hear it, as when its connection opens.
    func tell(_ core: String) {
        let agentId = inFront ? shown[core] : nil
        let params = OnScreenParams(agentIds: agentId.map { [$0] } ?? [], focused: agentId != nil)
        guard sent[core] != params, let connection = Hub.shared.live(core).connection else { return }
        sent[core] = params
        Task {
            do {
                _ = try await connection.call(PhoneCall.OnScreen.self, params)
            } catch {
                sent[core] = nil
                log.error("on screen: \(problem(error), privacy: .public)")
            }
        }
    }
}
