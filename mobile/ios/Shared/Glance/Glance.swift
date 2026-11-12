import ActivityKit
import Foundation

/// What the phone's widgets and Live Activity draw, as the app last put together from every host's view.
struct Glance: Codable, Hashable, Sendable {
    var agents: AgentsGlance
    var limits: [LimitGlance]
}

/// The agents that need the person or work now, across every host, in the order the hosts rank them.
struct AgentsGlance: Codable, Hashable, Sendable {
    /// The first few, those that need the person first.
    var rows: [AgentGlance]
    var needed: Int
    var working: Int

    static let none = AgentsGlance(rows: [], needed: 0, working: 0)

    var active: Bool { needed > 0 || working > 0 }
}

struct AgentGlance: Codable, Hashable, Sendable, Identifiable {
    enum State: String, Codable, Sendable {
        case needs, working, done, failed
    }

    var host: String
    var hostName: String
    var agentId: String
    var cli: String
    var title: String
    var name: String?
    var state: State

    var id: String { "\(host)/\(agentId)" }
}

/// One rolling window of an agent account's plan.
struct LimitGlance: Codable, Hashable, Sendable, Identifiable {
    var provider: String
    var account: String
    var plan: String?
    var window: String
    var used: Double
    var resetsAt: Date?
    var readAt: Date?
    /// The last reading failed or is well past due, so the numbers may be old.
    var stale: Bool

    var id: String { "\(provider)/\(account)/\(window)" }
}

/// What a Live Activity of the agents carries: who is at work across hosts, and nothing that identifies the phone.
struct AgentsActivity: ActivityAttributes {
    typealias ContentState = AgentsGlance
}

/// Where the app leaves the glance for its widgets: a file in the App Group both read.
enum GlanceStore {
    private static let fileName = "glance.json"

    static let agentsWidget = "sikemux.needs-you"
    static let limitsWidget = "sikemux.limits"

    private static func file() -> URL? {
        guard let group = Bundle.main.object(forInfoDictionaryKey: "SikemuxAppGroup") as? String else { return nil }
        return FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: group)?
            .appendingPathComponent(fileName)
    }

    static func read() -> Glance? {
        guard let file = file(), let data = try? Data(contentsOf: file) else { return nil }
        return try? JSONDecoder().decode(Glance.self, from: data)
    }

    static func write(_ glance: Glance?) {
        guard let file = file() else { return }
        do {
            if let glance {
                try JSONEncoder().encode(glance).write(
                    to: file, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
            } else if FileManager.default.fileExists(atPath: file.path) {
                try FileManager.default.removeItem(at: file)
            }
        } catch {
            print("sikemux: could not write what the widgets read: \(error)")
        }
    }

    /// A link into the app, in this build's own scheme.
    static func link(_ path: String) -> URL? {
        guard let scheme = Bundle.main.object(forInfoDictionaryKey: "SikemuxURLScheme") as? String else { return nil }
        return URL(string: "\(scheme)://\(path)")
    }
}
