import ActivityKit
import Foundation
import WidgetKit

/// Keeps the widgets and the agents' Live Activity in step with what the hosts send: who needs the person and who
/// works, from each host's view, and each account's plan limits, from `agentUsages`.
@MainActor
enum Glances {
    private static let rows = 4
    /// Hosts change their agents in bursts; the glance follows once they settle.
    private static let settle: Duration = .seconds(1)
    /// Limits change slowly, and the Mac reads them this often too.
    private static let limitsEvery: TimeInterval = 5 * 60
    /// With nothing new from the hosts this long, the Live Activity says its numbers may be old.
    private static let staleAfter: TimeInterval = 15 * 60
    /// The Live Activity stays on the lock screen this long after the last agent stops.
    private static let lingers: TimeInterval = 5 * 60

    private static var written: Glance?
    private static var settling: Task<Void, Never>?
    private static var signedIn = false
    private static var usages: [String: [AccountUsage]] = [:]
    private static var usagesRead: [String: Date] = [:]

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
        GlanceStore.write(nil)
        WidgetCenter.shared.reloadAllTimelines()
        Task { await endActivities(.none) }
    }

    /// A host sent a new view, or the app came to the front.
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
        let next = Glance(agents: agents(), limits: limits())
        guard next != written else { return }
        let agentsMoved = next.agents != written?.agents
        let limitsMoved = next.limits != written?.limits
        written = next
        GlanceStore.write(next)
        if agentsMoved { WidgetCenter.shared.reloadTimelines(ofKind: GlanceStore.agentsWidget) }
        if limitsMoved { WidgetCenter.shared.reloadTimelines(ofKind: GlanceStore.limitsWidget) }
        if agentsMoved { await show(next.agents) }
    }

    private static func agents() -> AgentsGlance {
        var needs: [AgentGlance] = []
        var working: [AgentGlance] = []
        var needed = 0
        var workingCount = 0
        let hidden = Lock.shared.on
        for device in Hub.shared.devices {
            guard let inbox = Hub.shared.live(device.core).view?.inbox else { continue }
            needed += inbox.needed ?? inbox.items.filter { $0.need != nil }.count
            workingCount += inbox.working
            for agent in inbox.agents {
                let row = { (state: AgentGlance.State) in
                    AgentGlance(
                        host: device.core, hostName: device.displayName, agentId: agent.agentId, cli: agent.cli,
                        title: hidden ? "" : agent.title, name: hidden ? nil : agent.name, state: state)
                }
                switch agent.group {
                case .blocked: needs.append(row(.needs))
                case .working: working.append(row(.working))
                case .done, .idle, .unknown: break
                }
            }
        }
        return AgentsGlance(rows: Array((needs + working).prefix(rows)), needed: needed, working: workingCount)
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

    private static func show(_ agents: AgentsGlance) async {
        let content = ActivityContent(state: agents, staleDate: Date().addingTimeInterval(staleAfter))
        let running = Activity<AgentsActivity>.activities
        guard agents.active else { return await endActivities(agents) }
        if let first = running.first {
            await first.update(content)
            for extra in running.dropFirst() { await extra.end(nil, dismissalPolicy: .immediate) }
            return
        }
        guard ActivityAuthorizationInfo().areActivitiesEnabled else { return }
        do {
            _ = try Activity.request(attributes: AgentsActivity(), content: content)
        } catch {
            print("sikemux: could not start the agents' Live Activity: \(error)")
        }
    }

    private static func endActivities(_ last: AgentsGlance) async {
        let content = ActivityContent(state: last, staleDate: nil)
        for activity in Activity<AgentsActivity>.activities {
            await activity.end(content, dismissalPolicy: .after(Date().addingTimeInterval(lingers)))
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
