import Foundation

/// How a host is doing, in the words every screen uses for it.
struct HostStatus: Equatable {
    /// Connected now; anything shown from an older connection is stale rather than live.
    var online: Bool
    var connecting: Bool
    /// What is shown is from a connection that has ended, so it may be out of date.
    var stale: Bool
    /// The host no longer knows this phone; only forgetting it and joining again helps.
    var unpaired: Bool
    var outdated: Outdated?
    var line: String
    var problem: String?

    @MainActor
    init(_ live: HostLive, device: PairedDevice?, now: Date = Date()) {
        online = live.status == .open
        stale = !online && live.view != nil
        unpaired = false
        outdated = nil
        problem = nil
        connecting = false
        let offline =
            "Asleep or offline"
            + (device?.lastSeen.map { " · seen \(Format.ago(Format.date(millis: $0), now: now))" } ?? "")
        switch live.status {
        case .open:
            line = ["Online", device?.channelLabel, live.view.map(Self.summary)].compactMap { $0 }.joined(
                separator: " · ")
        case .connecting(let failed):
            connecting = true
            problem = failed
            // Trying again after a failure still reads as offline, so the line does not flicker on every try.
            line = failed != nil ? offline : stale ? "Reconnecting…" : "Connecting…"
        case .closed(let reason, let outdatedBy, let unpairedNow):
            outdated = outdatedBy
            unpaired = unpairedNow
            problem = reason
            switch outdatedBy {
            case .host: line = "Needs a newer Sikemux"
            case .phone: line = "Update this app to connect"
            case nil: line = unpairedNow ? "No longer knows this phone" : offline
            }
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
