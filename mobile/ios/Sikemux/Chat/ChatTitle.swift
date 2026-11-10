import SwiftUI

/// A chat's header: its agent's mark, its title, then its name dimmed, as the Mac's header reads; the same before and
/// after the chat starts.
struct ChatTitle: View {
    @Environment(\.palette) private var palette
    var agentId: String?
    var group: AgentGroup?
    let provider: String
    var name: String?
    let title: String

    var body: some View {
        AgentFaceView(agentId: agentId, provider: provider, group: group, size: 17)
        Text(title).font(Theme.ui(16, .semibold)).tracking(-0.25).foregroundStyle(palette.ink).lineLimit(1)
            .accessibilityAddTraits(.isHeader)
        if let name { Text("@\(name)").font(Theme.ui(12)).foregroundStyle(palette.tertiary).fixedSize() }
    }
}
