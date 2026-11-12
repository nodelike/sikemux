import SwiftUI
import WidgetKit

/// How many agents wait on the person across hosts, and the first of them.
struct NeedsYouWidget: Widget {
    var body: some WidgetConfiguration {
        StaticConfiguration(kind: GlanceStore.agentsWidget, provider: GlanceTimeline()) { entry in
            NeedsYouView(glance: entry.glance)
                .containerBackground(palette.sunken, for: .widget)
        }
        .configurationDisplayName("Needs you")
        .description("The agents waiting on you, across your hosts.")
        .supportedFamilies([.systemSmall, .systemMedium])
    }
}

private struct NeedsYouView: View {
    let glance: Glance?
    @Environment(\.widgetFamily) private var family

    var body: some View {
        let hosts = glance?.hosts ?? []
        let hidden = glance?.hidden ?? false
        let needed = hosts.reduce(0) { $0 + $1.agents.needed }
        let working = hosts.reduce(0) { $0 + $1.agents.working }
        let waiting = hosts.flatMap { host in host.agents.rows.map { (host: host, row: $0) } }
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                BrandMark(size: 12)
                Text("Needs you").font(Theme.ui(11.5)).foregroundStyle(palette.tertiary)
            }
            Text("\(needed)")
                .font(Theme.ui(38, .semibold))
                .foregroundStyle(needed > 0 ? palette.warn : palette.tertiary)
            if waiting.isEmpty {
                Text(working > 0 ? "\(working) working" : "Nothing waits on you")
                    .font(Theme.ui(11.5)).foregroundStyle(palette.secondary)
            }
            ForEach(waiting.prefix(family == .systemSmall ? 2 : 3), id: \.row.agentId) { waiting in
                Link(destination: chatLink(host: waiting.host.host, agentId: waiting.row.agentId)) {
                    HStack(spacing: 5) {
                        AgentMark(provider: waiting.row.cli, size: 12)
                        Text(line(waiting.row, hostName: waiting.host.hostName, hidden: hidden))
                            .font(Theme.ui(11.5)).foregroundStyle(palette.ink).lineLimit(1)
                    }
                }
            }
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func line(_ row: ActivityRow, hostName: String, hidden: Bool) -> String {
        if hidden { return hostName }
        return row.name.map { "@\($0) · \(row.title)" } ?? row.title
    }
}
