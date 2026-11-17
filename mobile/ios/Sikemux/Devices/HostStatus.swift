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
            device?.lastSeen.map {
                String(localized: "Asleep or offline · seen \(Format.ago(Format.date(millis: $0), now: now))")
            } ?? String(localized: "Asleep or offline")
        switch live.status {
        case .open:
            facts = [device?.channelLabel, live.view.map(Self.summary)].compactMap { $0 }.joined(separator: " · ")
            line = [String(localized: "Online"), facts].filter { !$0.isEmpty }.joined(separator: " · ")
        case .connecting(let failed):
            connecting = true
            problem = failed
            // Trying again after a failure still reads as offline, so the line does not flicker on every try.
            line =
                failed != nil ? offline : stale ? String(localized: "Reconnecting…") : String(localized: "Connecting…")
            facts = line
        case .closed(let reason, let why):
            blocked = why
            problem = reason
            switch why {
            case .outdated(.host): line = String(localized: "Needs a newer Sikemux")
            case .outdated(.phone): line = String(localized: "Update this app to connect")
            case .unpaired: line = String(localized: "No longer knows this phone")
            case nil: line = offline
            }
            facts = line
        }
    }

    static func summary(_ view: DeviceView) -> String {
        let agents = view.chats.count
        let terminals = view.sessions.filter(\.running).count
        var parts: [String] = []
        if agents > 0 {
            parts.append(agents == 1 ? String(localized: "1 agent") : String(localized: "\(agents) agents"))
        }
        if terminals > 0 {
            parts.append(
                terminals == 1 ? String(localized: "1 terminal") : String(localized: "\(terminals) terminals"))
        }
        return parts.isEmpty ? String(localized: "Nothing running") : parts.joined(separator: " · ")
    }
}
