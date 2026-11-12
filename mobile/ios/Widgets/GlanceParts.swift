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

/// Something waits on the person: the rail's amber dot, still.
struct NeedsMark: View {
    var body: some View {
        Circle().fill(palette.warn).frame(width: 7, height: 7).frame(width: 13, height: 13)
            .accessibilityLabel("Needs you")
    }
}

struct BrandMark: View {
    var size: CGFloat = 14

    var body: some View { IconView(icon: Icons.brand, size: size).foregroundStyle(palette.accent) }
}

/// "2 need you · 27 working · 3 done", with the numbers in full ink.
struct AgentCounts: View {
    let glance: ActivityGlance

    var body: some View {
        let parts = [
            glance.needed > 0 ? (glance.needed, glance.needed == 1 ? "needs you" : "need you") : nil,
            glance.working > 0 ? (glance.working, "working") : nil,
            glance.done > 0 ? (glance.done, "done") : nil,
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

/// What needs the person, as the rail draws it: the mark, the title, the name small at the end with how many of a
/// lead's team work, then the amber dot. With the lock on it shows the host in place of the title, and no name.
struct NeedsLine: View {
    let row: ActivityRow
    let hostName: String
    let hidden: Bool
    var size: CGFloat = 13.5

    var body: some View {
        HStack(spacing: 8) {
            AgentMark(provider: row.cli, size: size + 3)
            Text(hidden ? hostName : row.title)
                .font(Theme.ui(size))
                .foregroundStyle(palette.ink)
                .lineLimit(1)
                .frame(maxWidth: .infinity, alignment: .leading)
            HStack(spacing: 4) {
                if let name = row.name, !hidden { Text("@\(name)") }
                if row.teamWorking > 0 {
                    Text(hidden ? "\(row.teamWorking)" : "· \(row.teamWorking)")
                    WorkingMark()
                }
            }
            .font(Theme.ui(size - 1.5)).foregroundStyle(palette.tertiary).lineLimit(1)
            NeedsMark()
        }
    }
}

/// A project most at work, while nothing needs the person.
struct ProjectLine: View {
    let project: ActivityProject
    var size: CGFloat = 13.5

    var body: some View {
        HStack(spacing: 8) {
            IconView(icon: Icons.folder, size: size + 1).foregroundStyle(palette.tertiary).frame(width: size + 3)
            Text(project.name).font(Theme.ui(size)).foregroundStyle(palette.ink).lineLimit(1)
                .frame(maxWidth: .infinity, alignment: .leading)
            Text("\(project.working) working").font(Theme.ui(size - 1.5)).foregroundStyle(palette.tertiary)
            WorkingMark()
        }
    }
}

/// The rows of a glance: what needs the person and how many more, else the projects most at work.
struct GlanceRows: View {
    let glance: ActivityGlance
    let host: String
    let hostName: String
    let hidden: Bool
    let limit: Int
    var height: CGFloat = 30

    var body: some View {
        if glance.rows.isEmpty {
            ForEach(glance.projects.prefix(limit)) { ProjectLine(project: $0).frame(height: height) }
        } else {
            ForEach(glance.rows.prefix(limit)) { row in
                Link(destination: chatLink(host: host, agentId: row.agentId)) {
                    NeedsLine(row: row, hostName: hostName, hidden: hidden).frame(height: height)
                }
            }
            let more = glance.more + max(0, glance.rows.count - limit)
            if more > 0 {
                Text("+\(more) more \(more == 1 ? "needs" : "need") you")
                    .font(Theme.ui(12)).foregroundStyle(palette.tertiary).padding(.leading, 26)
            }
            StateMark(state: agent.state)
        }
    }
}

/// Where tapping an agent goes: its chat on its host.
func chatLink(host: String, agentId: String) -> URL {
    GlanceStore.link("device/\(host)/chat/\(agentId)") ?? URL(fileURLWithPath: "/")
}
