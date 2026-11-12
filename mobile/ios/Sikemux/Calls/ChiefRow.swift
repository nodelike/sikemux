import SikemuxCore
import SwiftUI

/// A project's chief, @Chief, first among its agents: drawn from the project itself, whether or not its chat exists
/// yet. A tap opens its chat, or asks the host for it, which makes it the first time and brings a closed one back.
struct ChiefRow: View {
    @Environment(\.palette) private var palette
    let core: String
    let project: ProjectInfo
    /// The chief's own row in the host's list, while its chat is open.
    let shown: AgentShown?
    /// Agents of several projects share the list, so the row names its project at its end.
    let mixed: Bool
    let now: Date
    @State private var opening = false
    @State private var refused: String?
    /// The row is on screen, so the chat it asked for may open over it.
    @State private var here = false

    var body: some View {
        Row(
            title: CallWords.chief, aside: shown?.titled == true ? shown?.title : nil, detail: refused,
            onPress: open,
            mark: { AskerMark(agentId: shown?.agentId ?? "", provider: shown?.cli ?? "", chief: true) },
            end: {
                HStack(spacing: 8) {
                    if mixed {
                        HStack(spacing: 4) {
                            IconView(icon: Icons.folder, size: 11).foregroundStyle(palette.live)
                            Text(project.name).font(Theme.ui(12)).foregroundStyle(palette.tertiary).lineLimit(1)
                        }
                    }
                    if opening {
                        Working()
                    } else if let shown {
                        TurnMark(turn: shown.turn, now: now)
                    }
                }
            }
        )
        .onAppear { here = true }
        .onDisappear { here = false }
    }

    private func open() {
        if let shown {
            Router.shared.push(.chat(core: core, agent: shown.agentId))
            return
        }
        guard !opening else { return }
        guard let connection = Hub.shared.live(core).connection else {
            Haptics.failure()
            refused = CallWords.chiefOffline
            return
        }
        opening = true
        refused = nil
        Task {
            defer { opening = false }
            do {
                let opened = try await connection.call(PhoneCall.OpenChief.self, OpenChiefParams(project: project.id))
                if here { Router.shared.push(.chat(core: core, agent: opened.agentId)) }
            } catch {
                Haptics.failure()
                refused = problem(error)
            }
        }
    }
}
