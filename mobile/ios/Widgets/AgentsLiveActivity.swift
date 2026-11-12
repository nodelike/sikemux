import ActivityKit
import SwiftUI
import WidgetKit

/// One host's agents on the lock screen and in the Dynamic Island, however many: the counts, then what needs the
/// person, else the projects most at work.
struct AgentsLiveActivity: Widget {
    var body: some WidgetConfiguration {
        ActivityConfiguration(for: AgentsActivity.self) { context in
            LockCard(
                glance: context.state.opened(host: context.attributes.host), host: context.attributes.host,
                hostName: context.attributes.hostName, stale: context.isStale
            )
            .activityBackgroundTint(palette.sunken.opacity(0.86))
            .activitySystemActionForegroundColor(palette.ink)
        } dynamicIsland: { context in
            let host = context.attributes.host
            let glance = context.state.opened(host: host) ?? ActivityGlance.quiet
            return DynamicIsland {
                DynamicIslandExpandedRegion(.bottom) {
                    VStack(alignment: .leading, spacing: 6) {
                        HStack(spacing: 8) {
                            BrandMark()
                            AgentCounts(glance: glance)
                        }
                        GlanceRows(
                            glance: glance, host: host, hostName: context.attributes.hostName,
                            hidden: GlanceStore.read()?.hidden ?? false, limit: 2, height: 26)
                    }
                    .opacity(context.isStale ? 0.5 : 1)
                }
            } compactLeading: {
                if glance.needed > 0 { Count(count: glance.needed) { NeedsMark() } }
            } compactTrailing: {
                if glance.working > 0 { Count(count: glance.working) { WorkingMark() } }
            } minimal: {
                if glance.needed > 0 { NeedsMark() } else { WorkingMark() }
            }
            .keylineTint(palette.accent)
        }
    }
}

private struct LockCard: View {
    let glance: ActivityGlance?
    let host: String
    let hostName: String
    let stale: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                BrandMark()
                if let glance {
                    AgentCounts(glance: glance)
                } else {
                    Text("Agents at work").font(Theme.ui(13)).foregroundStyle(palette.secondary)
                }
                Spacer(minLength: 0)
                Text(stale ? "May be out of date" : hostName)
                    .font(Theme.ui(12)).foregroundStyle(palette.tertiary).lineLimit(1)
            }
            .padding(.bottom, 6)
            if let glance {
                GlanceRows(
                    glance: glance, host: host, hostName: hostName, hidden: GlanceStore.read()?.hidden ?? false,
                    limit: 3)
            }
        }
        .opacity(stale ? 0.6 : 1)
        .padding(.horizontal, 14)
        .padding(.vertical, 12)
    }
}

/// A count beside its mark, as the notch's wings show them.
private struct Count<Mark: View>: View {
    let count: Int
    @ViewBuilder let mark: () -> Mark

    var body: some View {
        HStack(spacing: 5) {
            Text("\(count)").font(Theme.ui(12, .semibold)).monospacedDigit().foregroundStyle(palette.ink)
            mark()
        }
    }
}

extension ActivityGlance {
    /// What shows before an update can be opened: nothing counted.
    static let quiet = ActivityGlance(needed: 0, working: 0, done: 0, rows: [], more: 0, projects: [])
}
