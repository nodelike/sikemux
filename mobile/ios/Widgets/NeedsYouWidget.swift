import SwiftUI
import WidgetKit

/// How many agents wait on the person across hosts, and the first of them.
struct NeedsYouWidget: Widget {
    var body: some WidgetConfiguration {
        StaticConfiguration(kind: GlanceStore.agentsWidget, provider: GlanceTimeline()) { entry in
            NeedsYouView(agents: entry.glance?.agents ?? .none)
                .containerBackground(palette.sunken, for: .widget)
        }
        .configurationDisplayName("Needs you")
        .description("The agents waiting on you, across your hosts.")
        .supportedFamilies([.systemSmall, .systemMedium])
    }
}

private struct NeedsYouView: View {
    let agents: AgentsGlance
    @Environment(\.widgetFamily) private var family

    var body: some View {
        let waiting = agents.rows.filter { $0.state == .needs }
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                BrandMark(size: 12)
                Text("Needs you").font(Theme.ui(11.5)).foregroundStyle(palette.tertiary)
            }
            Text("\(agents.needed)")
                .font(Theme.ui(38, .semibold))
                .foregroundStyle(agents.needed > 0 ? palette.warn : palette.tertiary)
            if waiting.isEmpty {
                Text(agents.working > 0 ? "\(agents.working) working" : "Nothing waits on you")
                    .font(Theme.ui(11.5)).foregroundStyle(palette.secondary)
            }
            ForEach(waiting.prefix(family == .systemSmall ? 2 : 3)) { agent in
                Link(destination: agent.link ?? URL(fileURLWithPath: "/")) {
                    HStack(spacing: 5) {
                        AgentMark(provider: agent.cli, size: 12)
                        Text(agent.title.isEmpty ? agent.hostName : agent.title)
                            .font(Theme.ui(11.5)).foregroundStyle(palette.ink).lineLimit(1)
                    }
                }
            }
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}
