import SwiftUI
import WidgetKit

/// An agent at work, as the rail's squares stand still: a widget can't animate them.
struct WorkingMark: View {
    var body: some View {
        Canvas { context, _ in
            for index in 0..<9 {
                let rect = CGRect(x: Double(index % 3) * 5, y: Double(index / 3) * 5, width: 3, height: 3)
                context.opacity = [1, 0.5, 0.8, 0.4, 1, 0.6, 0.9, 0.5, 0.7][index]
                context.fill(Path(roundedRect: rect, cornerRadius: 0.6), with: .color(palette.live))
            }
        }
        .frame(width: 13, height: 13)
        .accessibilityLabel("Working")
    }
}

struct StateMark: View {
    let state: AgentGlance.State

    var body: some View {
        switch state {
        case .needs: Circle().fill(palette.warn).frame(width: 7, height: 7).frame(width: 13, height: 13)
        case .working: WorkingMark()
        case .done: Circle().fill(palette.accent).frame(width: 7, height: 7).frame(width: 13, height: 13)
        case .failed: Circle().fill(palette.danger).frame(width: 7, height: 7).frame(width: 13, height: 13)
        }
    }
}

struct BrandMark: View {
    var size: CGFloat = 14

    var body: some View { IconView(icon: Icons.brand, size: size).foregroundStyle(palette.accent) }
}

/// "1 needs you · 3 working", with the numbers in full ink.
struct AgentCounts: View {
    let agents: AgentsGlance

    var body: some View {
        let parts = [
            agents.needed > 0 ? (agents.needed, "needs you") : nil,
            agents.working > 0 ? (agents.working, "working") : nil,
        ].compactMap { $0 }
        parts.enumerated().reduce(Text("")) { text, part in
            let (index, (count, word)) = part
            return text + Text(index > 0 ? " · " : "") + Text("\(count)").foregroundStyle(palette.ink).bold()
                + Text(" \(word)")
        }
        .font(Theme.ui(13))
        .foregroundStyle(palette.secondary)
        .lineLimit(1)
    }
}

/// One agent as the rail draws it: its mark, its title, its name small at the end, then its state.
struct AgentLine: View {
    let agent: AgentGlance
    var size: CGFloat = 13.5

    var body: some View {
        HStack(spacing: 8) {
            AgentMark(provider: agent.cli, size: size + 3)
            Text(agent.title.isEmpty ? agent.hostName : agent.title)
                .font(Theme.ui(size))
                .foregroundStyle(palette.ink)
                .lineLimit(1)
                .frame(maxWidth: .infinity, alignment: .leading)
            if let name = agent.name {
                Text("@\(name)").font(Theme.ui(size - 1.5)).foregroundStyle(palette.tertiary).lineLimit(1)
            }
            StateMark(state: agent.state)
        }
    }
}

extension AgentGlance {
    var link: URL? { GlanceStore.link("device/\(host)/chat/\(agentId)") }
}
