import Foundation

/// How a host is doing, in the words every screen uses for it.
struct HostStatus: Equatable {
    /// Connected now; anything shown from an older connection is stale rather than live.
    var online: Bool
    var connecting: Bool
    /// What is shown is from a connection that has ended, so it may be out of date.
    var stale: Bool
    var blocked: HostLive.Blocked?
    var unpaired: Bool { blocked == .unpaired }
    var outdated: Outdated? {
        if case .outdated(let side) = blocked { side } else { nil }
    }
    var line: String
    /// The line without saying it is online, for a screen whose dot already says so.
    var facts: String
    var problem: String?

    @MainActor
    init(_ live: HostLive, device: PairedDevice?, now: Date = Date()) {
        online = live.status == .open
        stale = !online && live.view != nil
        blocked = nil
        problem = nil
        connecting = false
        let offline =
            "Asleep or offline"
            + (device?.lastSeen.map { " · seen \(Format.ago(Format.date(millis: $0), now: now))" } ?? "")
        switch live.status {
        case .open:
            facts = [device?.channelLabel, live.view.map(Self.summary)].compactMap { $0 }.joined(separator: " · ")
            line = ["Online", facts].filter { !$0.isEmpty }.joined(separator: " · ")
        case .connecting(let failed):
            connecting = true
            problem = failed
            // Trying again after a failure still reads as offline, so the line does not flicker on every try.
            line = failed != nil ? offline : stale ? "Reconnecting…" : "Connecting…"
            facts = line
        case .closed(let reason, let why):
            blocked = why
            problem = reason
            switch why {
            case .outdated(.host): line = "Needs a newer Sikemux"
            case .outdated(.phone): line = "Update this app to connect"
            case .unpaired: line = "No longer knows this phone"
            case nil: line = offline
            }
            facts = line
        }
    }

    static func summary(_ view: DeviceView) -> String {
        let agents = view.chats.count
        let terminals = view.sessions.filter(\.running).count
        var parts: [String] = []
        if agents > 0 { parts.append("\(agents) \(Format.counted("agent", agents))") }
        if terminals > 0 { parts.append("\(terminals) \(Format.counted("terminal", terminals))") }
        return parts.isEmpty ? "Nothing running" : parts.joined(separator: " · ")
    }
}
