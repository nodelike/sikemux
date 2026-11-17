import SwiftUI

/// The chat's last line once a turn has ended, or while it works on a step it named: the state's mark, how the turn
/// stands in the core's words, and the next steps as buttons on the same row. A failed turn's line is the trouble
/// card's. With the call it waits on docked right under it, the line says only how long it has waited.
struct TurnLine: View {
    @Environment(\.palette) private var palette
    @Environment(\.crew) private var crew
    let core: String
    let agentId: String
    let provider: String
    /// The project's chief, which wears Sikemux's S.
    let chief: Bool
    let turn: TurnState
    let canAct: Bool
    /// The call the turn waits on is docked on this chat.
    let docked: Bool
    let onLook: () -> Void
    @State private var taking: TurnStep?
    @State private var refused: String?

    var body: some View {
        TimelineView(.periodic(from: Self.minuteStart, by: ticks ? 1 : 60)) { timeline in
            if turn.state != .failed,
                let said = CallWords.said(turn, maker: Catalog.label(provider, core: core), now: timeline.date)
            {
                VStack(alignment: .leading, spacing: 4) {
                    HStack(spacing: 8) {
                        OwnMark(agentId: agentId, provider: provider, chief: chief, turn: turn, size: 18)
                        words(docked ? CallWords.docked(said, waiting: turn.call?.count ?? 1) : said).frame(
                            maxWidth: .infinity, alignment: .leading)
                        if canAct { buttons }
                    }
                    .frame(minHeight: 32)
                    if let refused {
                        Text(refused).font(Theme.ui(12.5)).foregroundStyle(palette.danger).padding(.leading, 28)
                    }
                }
                .padding(.top, 10)
            }
        }
    }

    /// Only a working turn counts seconds; the other states show minutes, on the same minute as the call card's age.
    private var ticks: Bool { turn.state == .working }

    private static var minuteStart: Date { Calendar.current.dateInterval(of: .minute, for: .now)?.start ?? .now }

    /// The most agents the line names by their chips; the rest are counted in one chip that lists them.
    private static let shownAgents = 2

    /// The agents the turn waits on that this host lists, longest at work first.
    private var waitedOn: [Crew.Named] {
        guard let wait = turn.wait, wait.kind == .agent, let crew else { return [] }
        let ids = wait.agents ?? wait.agent.map { [$0] } ?? []
        return ids.compactMap { crew.agent($0) }
    }

    private func words(_ said: CallWords.Said) -> some View {
        HStack(spacing: 6) {
            if said.phase == .waiting, !waitedOn.isEmpty {
                Text("Waiting on").font(Theme.ui(13, .medium)).foregroundStyle(tint(said.phase)).fixedSize()
                ForEach(waitedOn.prefix(Self.shownAgents), id: \.id) { agent in
                    AgentPill(agent: agent, size: 12.5).fixedSize()
                }
                if waitedOn.count > Self.shownAgents {
                    MoreAgents(agents: Array(waitedOn.dropFirst(Self.shownAgents))).fixedSize()
                }
            } else {
                // A wait's place, holder and minutes matter more than its long name, which gives way first.
                Text(said.word).font(Theme.ui(13, .medium)).foregroundStyle(tint(said.phase)).lineLimit(2)
                    .layoutPriority(said.phase == .waiting ? -1 : 0)
            }
            ForEach(Array(said.parts.enumerated()), id: \.offset) { index, part in
                Text("·").foregroundStyle(palette.inkFaint).fixedSize()
                Text(part)
                    .foregroundStyle(index == 0 ? palette.secondary : palette.tertiary)
                    .lineLimit(1)
                    .layoutPriority(index == said.parts.count - 1 ? 1 : 0)
                    .fixedSize(horizontal: index == said.parts.count - 1, vertical: false)
            }
        }
        .font(Theme.ui(12.5))
        .accessibilityElement(children: .combine)
    }

    private func tint(_ phase: TurnPhase) -> Color {
        switch phase {
        case .done: palette.accent
        case .waiting: palette.cmd
        case .needsYou: palette.warn
        case .failed: palette.danger
        case .stopped: palette.secondary
        case .working, .unknown: palette.ink
        }
    }

    @ViewBuilder
    private var buttons: some View {
        switch turn.state {
        case .done:
            HStack(spacing: 6) {
                ForEach(turn.done?.next ?? [], id: \.step) { next in
                    stepChip(next.step, primary: next.primary)
                }
            }
        case .needsYou where !docked:
            Chip(title: CallWords.look, primary: true, action: onLook)
        case .stopped:
            stepChip(.carryOn, primary: false)
        default:
            EmptyView()
        }
    }

    @ViewBuilder
    private func stepChip(_ step: TurnStep, primary: Bool) -> some View {
        if let label = CallWords.step(step) {
            Chip(title: label, primary: primary, busy: taking == step) { take(step) }
        }
    }

    /// Takes a step; a production step raises its call, which docks on this chat to be answered.
    private func take(_ step: TurnStep) {
        guard taking == nil, let connection = Hub.shared.live(core).connection else { return }
        taking = step
        refused = nil
        Task {
            defer { taking = nil }
            do {
                _ = try await connection.call(PhoneCall.TakeStep.self, TakeStepParams(agentId: agentId, step: step))
            } catch {
                Haptics.failure()
                refused = problem(error)
            }
        }
    }
}

/// The count of the agents a line waits on past the ones it names; a tap lists them as their chips.
private struct MoreAgents: View {
    @Environment(\.palette) private var palette
    let agents: [Crew.Named]
    @State private var open = false

    var body: some View {
        Button {
            Haptics.select()
            open = true
        } label: {
            Text("+\(agents.count)")
                .font(Theme.ui(12.5, .medium))
                .foregroundStyle(palette.secondary)
                .padding(.horizontal, 6)
                .frame(height: 20.5)
                .background(palette.active, in: RoundedRectangle(cornerRadius: Radius.control * 0.5))
        }
        .buttonStyle(.plain)
        .accessibilityLabel(agents.map(\.written).joined(separator: ", "))
        .popover(isPresented: $open) {
            VStack(alignment: .leading, spacing: 6) {
                ForEach(agents, id: \.id) { AgentPill(agent: $0, size: 13).fixedSize() }
            }
            .padding(12)
            .simultaneousGesture(TapGesture().onEnded { open = false })
            .presentationCompactAdaptation(.popover)
        }
    }
}
