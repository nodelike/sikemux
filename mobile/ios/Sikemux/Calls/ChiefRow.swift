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
    /// Agents of several projects share the list, so the row names its project.
    let mixed: Bool
    let now: Date
    @State private var opening = false
    @State private var refused: String?

    var body: some View {
        Row(
            title: CallWords.chief(mixed ? project.name : nil),
            detail: refused ?? detail,
            onPress: open,
            mark: { AskerMark(agentId: shown?.agentId ?? "", provider: shown?.cli ?? "", chief: true) },
            end: {
                if opening {
                    Working()
                } else if let shown {
                    TurnMark(turn: shown.turn, now: now)
                }
            }
        )
    }

    /// What its team holds, or that it waits for the person to write first.
    private var detail: String? {
        CallWords.chiefTeam(project.agents ?? 0)
    }

    private func open() {
        if let shown {
            Router.shared.push(.chat(core: core, agent: shown.agentId))
            return
        }
        guard !opening, let connection = Hub.shared.live(core).connection else { return }
        opening = true
        refused = nil
        Task {
            defer { opening = false }
            do {
                let opened = try await connection.call(PhoneCall.OpenChief.self, OpenChiefParams(project: project.id))
                Router.shared.push(.chat(core: core, agent: opened.agentId))
            } catch {
                Haptics.failure()
                refused = problem(error)
            }
        }
    }
}
