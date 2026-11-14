import ActivityKit
import Foundation
import WidgetKit

/// Keeps the widgets and each host's Live Activity in step with what the hosts send: who needs the person and who
/// works, from each host's view, and each account's plan limits, from `agentUsages`. Each Live Activity's push token
/// goes to its host, which keeps the activity current while the phone is locked.
@MainActor
enum Glances {
    /// Hosts change their agents in bursts; the glance follows once they settle.
    private static let settle: Duration = .seconds(1)
    /// Limits change slowly, and the Mac reads them this often too.
    private static let limitsEvery: TimeInterval = 5 * 60
    /// With nothing new this long, a Live Activity says its numbers may be old.
    private static let staleAfter: TimeInterval = 15 * 60
    /// A Live Activity stays on the lock screen this long after its last agent stops.
    private static let lingers: TimeInterval = 5 * 60

    private static var written: Glance?
    private static var settling: Task<Void, Never>?
    private static var signedIn = false
    private static var usages: [String: [AccountUsage]] = [:]
    private static var usagesRead: [String: Date] = [:]
    /// The push token of each host's Live Activity, as the phone last heard it.
    private static var tokens: [String: String] = [:]
    /// What each host was last told of its Live Activity's token, empty once told there is none, and on which
    /// connection, since a host forgets the token when it restarts.
    private static var handed: [String: (connection: ObjectIdentifier, token: String)] = [:]
    private static var following: Set<String> = []

    static func signIn() {
        signedIn = true
        changed()
    }

    static func signOut() {
        signedIn = false
        settling?.cancel()
        written = nil
        usages = [:]
        usagesRead = [:]
        handed = [:]
        tokens = [:]
        GlanceStore.write(nil)
        WidgetCenter.shared.reloadAllTimelines()
        Task {
            for activity in Activity<AgentsActivity>.activities { await activity.end(nil, dismissalPolicy: .immediate) }
        }
    }

    /// A host sent a new view, the lock was turned on or off, or the app came to the front.
    static func changed() {
        guard signedIn else { return }
        settling?.cancel()
        settling = Task {
            try? await Task.sleep(for: settle)
            guard !Task.isCancelled else { return }
            await readLimits()
            await write()
        }
    }

    private static func readLimits() async {
        let now = Date()
        for (core, connection) in Hub.shared.openConnections {
            guard now.timeIntervalSince(usagesRead[core] ?? .distantPast) >= limitsEvery else { continue }
            usagesRead[core] = now
            do {
                usages[core] = try await connection.call(PhoneCall.AgentUsages.self, NoParams())
            } catch {
                print("sikemux: could not read the plan limits of \(core.prefix(8)): \(error)")
            }
        }
    }

    private static func write() async {
        let next = Glance(hosts: hosts(), limits: limits(), hidden: Lock.shared.on)
        if next != written {
            let agentsMoved = next.hosts != written?.hosts || next.hidden != written?.hidden
            let limitsMoved = next.limits != written?.limits
            written = next
            GlanceStore.write(next)
            if agentsMoved { WidgetCenter.shared.reloadTimelines(ofKind: GlanceStore.agentsWidget) }
            if limitsMoved { WidgetCenter.shared.reloadTimelines(ofKind: GlanceStore.limitsWidget) }
            if agentsMoved { await show(next.hosts) }
        }
        await hand()
    }

    /// Each host's agents at a glance, as the host sends them.
    private static func hosts() -> [HostGlance] {
        Hub.shared.devices.compactMap { device in
            guard let agents = Hub.shared.live(device.core).view?.inbox.glance else { return nil }
            return HostGlance(host: device.core, hostName: device.displayName, agents: agents)
        }
    }

    /// Every signed-in account's windows once, however many hosts share it, from the newest reading.
    private static func limits() -> [LimitGlance] {
        var newest: [String: AccountUsage] = [:]
        for device in Hub.shared.devices {
            for account in usages[device.core] ?? [] where account.signedIn && account.usage != nil {
                let key = "\(account.provider)/\(account.label)"
                if (account.readAt ?? 0) >= (newest[key]?.readAt ?? -1) { newest[key] = account }
            }
        }
        return newest.values.sorted { ($0.provider, $0.label) < ($1.provider, $1.label) }.flatMap { account in
            (account.usage?.windows ?? []).map { window in
                LimitGlance(
                    provider: account.provider, account: account.label,
                    plan: account.usage?.tier ?? account.usage?.plan,
                    window: window.scope.map { "\(window.label) · \($0)" } ?? window.label,
                    used: window.usedPercent, resetsAt: window.resetsAt?.date,
                    readAt: account.readAt.map(Format.date(millis:)),
                    stale: account.problem != nil)
            }
        }
    }

    /// One Live Activity for each host where something works or waits, started while the app is in front.
    private static func show(_ hosts: [HostGlance]) async {
        for host in hosts {
            let content = ActivityContent(
                state: AgentsActivity.ContentState(glance: host.agents, sealed: nil),
                staleDate: Date().addingTimeInterval(staleAfter))
            let mine = { Activity<AgentsActivity>.activities.filter { $0.attributes.host == host.host } }
            if !host.agents.active {
                for activity in mine() {
                    await activity.end(content, dismissalPolicy: .after(Date().addingTimeInterval(lingers)))
                }
                continue
            }
            if let first = mine().first {
                await first.update(content)
                continue
            }
            guard ActivityAuthorizationInfo().areActivitiesEnabled else { continue }
            do {
                let activity = try Activity.request(
                    attributes: AgentsActivity(host: host.host, hostName: host.hostName), content: content,
                    pushType: .token)
                follow(activity)
            } catch {
                print("sikemux: could not start the Live Activity for \(host.host.prefix(8)): \(error)")
            }
        }
    }

    /// Hears each new push token of `activity`, and that it ended, to tell its host.
    private static func follow(_ activity: Activity<AgentsActivity>) {
        guard following.insert(activity.id).inserted else { return }
        Task {
            for await token in activity.pushTokenUpdates {
                tokens[activity.attributes.host] = token.map { String(format: "%02x", $0) }.joined()
                await hand()
            }
        }
        Task {
            for await state in activity.activityStateUpdates where state == .ended || state == .dismissed {
                tokens[activity.attributes.host] = nil
                await hand()
                following.remove(activity.id)
            }
        }
    }

    /// Tells each host it can reach the token of the Live Activity it keeps current, or that it ended.
    private static func hand() async {
        for activity in Activity<AgentsActivity>.activities { follow(activity) }
        for (core, connection) in Hub.shared.openConnections {
            let token = tokens[core]
            let told = (connection: ObjectIdentifier(connection), token: token ?? "")
            if let last = handed[core], last == told { continue }
            handed[core] = told
            do {
                _ = try await connection.call(PhoneCall.LiveActivity.self, ActivityParams(token: token))
            } catch {
                print("sikemux: could not tell \(core.prefix(8)) of its Live Activity: \(error)")
            }
        }
    }
}

extension UsageResetAt {
    var date: Date? {
        switch self {
        case .unix(let seconds): Date(timeIntervalSince1970: Double(seconds))
        case .iso(let text): Format.date(iso: text)
        }
    }
}
