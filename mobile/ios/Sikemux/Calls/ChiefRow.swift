import SikemuxCore
import SwiftUI

/// A project's chief, @Chief, first among its agents: drawn from the project itself, whether or not its chat exists
/// yet. A tap opens its chat, or asks the host for it, which makes it the first time and brings a closed one back.
/// A chief that lives on another host is tagged with that host and opens there, and while that host is out of reach
/// the row offers to take the chief over on this one.
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
        let away = project.chiefHome?.awaySince
        Row(
            title: CallWords.chief, host: homeTag, aside: CallWords.chiefRole,
            detail: refused ?? away.map { CallWords.outOfReach(ChatWords.clock($0, now: now)) },
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
                    } else if away != nil && Hub.shared.device(core)?.access == .full {
                        Button(CallWords.takeOver, action: takeOver)
                            .buttonStyle(.plain)
                            .font(Theme.ui(12.5, .medium))
                            .foregroundStyle(palette.ink)
                            .padding(.horizontal, 10)
                            .frame(height: 28)
                            .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(palette.borderStrong))
                    } else if let shown {
                        TurnMark(turn: shown.turn, now: now)
                    }
                }
            }
        )
        .onAppear { here = true }
        .onDisappear { here = false }
    }

    private var homeTag: String? {
        guard let home = project.chiefHome else { return nil }
        guard let since = home.awaySince else { return home.hostName }
        return CallWords.chiefAway(home.hostName, since: ChatWords.clock(since))
    }

    private func open() {
        if let shown {
            Router.shared.push(.chat(core: core, agent: shown.agentId))
            return
        }
        if let home = project.chiefHome {
            if Hub.shared.device(home.host) != nil {
                Router.shared.push(.chat(core: home.host, agent: home.agentId))
            } else {
                Haptics.failure()
                refused = CallWords.chiefElsewhere(home.hostName)
            }
            return
        }
        ask(PhoneCall.OpenChief.self)
    }

    private func takeOver() {
        ask(PhoneCall.TakeOverChief.self)
    }

    private func ask<C: PhoneCallable>(_ call: C.Type)
    where C.Params == OpenChiefParams, C.Answer == OpenedChief {
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
                let opened = try await connection.call(call, OpenChiefParams(project: project.id))
                if here { Router.shared.push(.chat(core: core, agent: opened.agentId)) }
            } catch {
                Haptics.failure()
                refused = problem(error)
            }
        }
    }
}
