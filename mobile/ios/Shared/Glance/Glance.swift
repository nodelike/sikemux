import ActivityKit
import Foundation

/// What the phone's widgets draw, as the app or the notification extension last left it.
struct Glance: Codable, Hashable, Sendable {
    var hosts: [HostGlance]
    var limits: [LimitGlance]
    /// The person turned the lock on, so nothing outside the app shows what agents work on.
    var hidden: Bool
}

/// One host's agents that need the person or work, as the host ranks them.
struct HostGlance: Codable, Hashable, Sendable, Identifiable {
    var host: String
    var hostName: String
    var agents: ActivityGlance

    var id: String { host }
}

extension ActivityGlance {
    var active: Bool { needed > 0 || working > 0 }
}

extension ActivityRow: Identifiable {
    var id: String { agentId }
}

extension ActivityProject: Identifiable {
    var id: String { name }
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
    /// The last reading failed, so the numbers may be old.
    var stale: Bool

    var id: String { "\(provider)/\(account)/\(window)" }
}

/// A Live Activity of one host's agents. The app fills it in while it runs; while the phone is locked the host
/// sends it sealed, and the widget extension opens it with the key the phone gave that host.
struct AgentsActivity: ActivityAttributes {
    struct ContentState: Codable, Hashable, Sendable {
        var glance: ActivityGlance?
        var sealed: String?

        /// What it shows: as the app put it, or as the host sealed it, once opened.
        func opened(host: String) -> ActivityGlance? {
            if let glance { return glance }
            let keys = Keys()
            guard let sealed, let phone = keys.phone,
                case .read(let from, let plaintext) = Envelope.open(sealed, phone: phone, keys: keys.all()),
                from == host
            else { return nil }
            return try? JSONDecoder().decode(ActivityGlance.self, from: Data(plaintext.utf8))
        }
    }

    var host: String
    var hostName: String
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
