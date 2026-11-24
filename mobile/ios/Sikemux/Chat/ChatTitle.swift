import SwiftUI

/// A chat's header, as the Mac's reads: the agents it works under, each a chip that opens it, then its own mark, its title
/// and its name dimmed. A chat with no title of its own yet shows its name in the title's place. Short of room, the
/// crumbs give way from the left.
struct ChatTitle: View {
    @Environment(\.palette) private var palette
    var agentId: String?
    var group: AgentGroup?
    /// How its turn stands, which the face wears in place of `group` where the core says it.
    var turn: TurnState?
    let provider: String
    var name: String?
    let title: String?
    /// The project's chief, which wears Sikemux's S.
    var chief = false
    /// The agents it works under, the chief first.
    var crumbs: [Crew.Named] = []

    var body: some View {
        HStack(spacing: 6) {
            if !crumbs.isEmpty {
                ViewThatFits(in: .horizontal) {
                    trail(crumbs, cut: false)
                    if crumbs.count > 1 { trail(Array(crumbs.suffix(1)), cut: true) }
                    trail([], cut: true)
                }
                .layoutPriority(1)
            }
            OwnMark(agentId: agentId, provider: provider, chief: chief, turn: turn, size: 17)
            words.frame(minWidth: crumbs.isEmpty ? nil : 64, alignment: .leading)
        }
    }

    @ViewBuilder private var words: some View {
        let handle = name.map { "@\($0)" } ?? (chief ? "@Chief" : nil)
        let heading = chief ? CallWords.chiefRole : title
        HStack(spacing: 6) {
            if let heading {
                Text(heading).font(Theme.ui(16, .semibold)).tracking(-0.25).foregroundStyle(palette.ink).lineLimit(1)
                    .accessibilityAddTraits(.isHeader)
                if let handle { Text(handle).font(Theme.ui(12)).foregroundStyle(palette.tertiary).fixedSize() }
            } else if let handle {
                Text(handle).font(Theme.ui(16, .semibold)).tracking(-0.25).foregroundStyle(palette.ink).lineLimit(1)
                    .accessibilityAddTraits(.isHeader)
            }
        }
    }

    private func trail(_ shown: [Crew.Named], cut: Bool) -> some View {
        HStack(spacing: 4) {
            if cut { Text("…").font(Theme.ui(13)).foregroundStyle(palette.tertiary) }
            ForEach(shown, id: \.id) { agent in
                AgentPill(agent: agent, size: 12.5).fixedSize()
                IconView(icon: Icons.chevron, size: 9).foregroundStyle(palette.inkFaint)
            }
            if cut && shown.isEmpty {
                IconView(icon: Icons.chevron, size: 9).foregroundStyle(palette.inkFaint)
            }
        }
        .fixedSize()
    }
}

extension Crew {
    /// The agents `agentId` works under, as the Mac's header names them: its project's chief, then the agent it works
    /// under.
    @MainActor static func crumbs(of agentId: String?, project: String?, in view: DeviceView?) -> [Named] {
        guard let view, let crew = Crew(view), let project,
            let chief = view.workspace.projects.first(where: { $0.id == project })?.chief, chief != agentId,
            let head = crew.agent(chief)
        else { return [] }
        let lead = view.inbox.agents.first { $0.agentId == agentId }?.lead
        return [head] + [crew.agent(lead)].compactMap { $0 }.filter { $0.id != chief }
    }
}
