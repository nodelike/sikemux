import OSLog

private let log = Logger(subsystem: "com.nodelike.sikemux", category: "on-screen")

/// Tells each host which of its chats this phone shows, as the Mac reports its screen: a chat shown stays awake, and one
/// shown live, the app in front and nothing locking it counts as read, and for the chief as the person being with it.
/// With the app behind or the phone locked, the host hears an empty list.
@MainActor
final class OnScreen {
    static let shared = OnScreen()
    private var shown: [String: (agentId: String, drawn: Bool)] = [:]
    private var sent: [String: (to: ObjectIdentifier, params: OnScreenParams)] = [:]
    private var inFront = true

    /// Chat `agentId` is on screen, on a phone with full access to the host, and read once it is `drawn` live.
    func show(_ core: String, _ agentId: String, drawn: Bool) {
        shown[core] = (agentId, drawn)
        tell(core)
    }

    func hide(_ core: String, _ agentId: String) {
        if shown[core]?.agentId == agentId { shown[core] = nil }
        tell(core)
    }

    /// The app came to the front, or went behind or the phone locked.
    func front(_ active: Bool) {
        inFront = active
        tellAll()
    }

    /// Says again what the phone shows, as when Sikemux's own lock covers it or opens.
    func tellAll() {
        for core in Set(shown.keys).union(sent.keys) { tell(core) }
    }

    /// Says it again once the host can hear it, as when its connection opens.
    func tell(_ core: String) {
        guard let connection = Hub.shared.live(core).connection else { return }
        let chat = inFront && Lock.shared.phase == .open ? shown[core] : nil
        let params = OnScreenParams(agentIds: chat.map { [$0.agentId] } ?? [], focused: chat?.drawn == true)
        // A new connection is a new client to the host, which has heard nothing from it yet.
        let to = ObjectIdentifier(connection)
        if let last = sent[core], last.to == to, last.params == params { return }
        sent[core] = (to, params)
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
