import ActivityKit
import SwiftUI
import WidgetKit

/// The agents at work on the lock screen and in the Dynamic Island, as the Mac's notch island shows them.
struct AgentsLiveActivity: Widget {
    var body: some WidgetConfiguration {
        ActivityConfiguration(for: AgentsActivity.self) { context in
            LockCard(agents: context.state, stale: context.isStale)
                .activityBackgroundTint(palette.sunken.opacity(0.86))
                .activitySystemActionForegroundColor(palette.ink)
        } dynamicIsland: { context in
            let agents = context.state
            return DynamicIsland {
                DynamicIslandExpandedRegion(.bottom) {
                    VStack(alignment: .leading, spacing: 6) {
                        HStack(spacing: 8) {
                            BrandMark()
                            AgentCounts(agents: agents)
                        }
                        ForEach(agents.rows.prefix(2)) { agent in
                            Link(destination: agent.link ?? URL(fileURLWithPath: "/")) { AgentLine(agent: agent) }
                        }
                    }
                    .opacity(context.isStale ? 0.5 : 1)
                }
            } compactLeading: {
                HStack(spacing: -2) {
                    ForEach(agents.rows.prefix(3)) { agent in AgentMark(provider: agent.cli, size: 16) }
                }
            } compactTrailing: {
                Rollup(agents: agents)
            } minimal: {
                StateMark(state: agents.needed > 0 ? .needs : .working)
            }
            .keylineTint(palette.accent)
        }
    }
}

private struct LockCard: View {
    let agents: AgentsGlance
    let stale: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                BrandMark()
                AgentCounts(agents: agents)
                Spacer(minLength: 0)
                if stale {
                    Text("May be out of date").font(Theme.ui(12)).foregroundStyle(palette.tertiary)
                } else if let host = hostName {
                    Text(host).font(Theme.ui(12)).foregroundStyle(palette.tertiary).lineLimit(1)
                }
            }
            .padding(.bottom, 6)
            ForEach(agents.rows.prefix(3)) { agent in
                Link(destination: agent.link ?? URL(fileURLWithPath: "/")) {
                    AgentLine(agent: agent).frame(height: 30)
                }
            }
        }
        .opacity(stale ? 0.6 : 1)
        .padding(.horizontal, 14)
        .padding(.vertical, 12)
    }

    /// The host, when every row is on one.
    private var hostName: String? {
        let hosts = Set(agents.rows.map(\.hostName))
        return hosts.count == 1 ? hosts.first : nil
    }
}

/// The notch's right wing: how many need the person with the amber dot, how many work with the squares.
private struct Rollup: View {
    let agents: AgentsGlance

    var body: some View {
        HStack(spacing: 5) {
            if agents.needed > 0 {
                Text("\(agents.needed)").font(Theme.ui(12, .semibold)).monospacedDigit().foregroundStyle(palette.ink)
                StateMark(state: .needs)
            }
            if agents.working > 0 {
                Text("\(agents.working)").font(Theme.ui(12, .semibold)).monospacedDigit().foregroundStyle(palette.ink)
                WorkingMark()
            }
        }
    }
}
