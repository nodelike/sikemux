import SwiftUI

/// A worker's finished work, as the Mac's lead chat shows it: its mark and name, what it worked on, where its work is and
/// how its checks stand, Try it and Undo, and a chevron that opens the worker's own chat.
struct ResultCard: View {
    @Environment(\.palette) private var palette
    @Environment(\.crew) private var crew
    @Environment(TranscriptContext.self) private var context
    let row: ResultRow
    @State private var taking: TurnStep?

    var body: some View {
        let named = crew?.agent(row.agentId)
        let turn = TurnState(state: .done, since: row.at, done: row.done)
        let facts = CallWords.said(turn, maker: Catalog.label(named?.provider ?? "agent", core: context.core))?.parts
            .joined(separator: " · ")
        let tries = row.done.next?.contains { $0.step == .tryIt } == true
        Button(action: open) {
            HStack(spacing: 8) {
                if let named {
                    NamedMark(agent: named, accent: palette.accent, size: 17)
                } else {
                    AgentMark(provider: "agent", size: 17)
                }
                Text(row.name.map { "@\($0)" } ?? row.title).font(Theme.ui(12.5, .semibold)).foregroundStyle(
                    palette.ink
                ).fixedSize()
                if row.name != nil {
                    Text(row.title).font(Theme.ui(12.5)).foregroundStyle(palette.inkDim).lineLimit(1).layoutPriority(-1)
                }
                if let facts, !facts.isEmpty {
                    HStack(spacing: 5) {
                        Dot(color: palette.accent, size: 7)
                        Text(facts).font(Theme.ui(12.5)).foregroundStyle(palette.inkDim).lineLimit(1)
                    }
                    .fixedSize()
                }
                Spacer(minLength: 0)
                if context.acts, tries { step(.tryIt, primary: true) }
                if context.acts, row.undo == true { step(.undo, primary: false) }
                IconView(icon: Icons.chevron, size: 11).foregroundStyle(palette.inkDim)
            }
            .padding(.leading, 12)
            .padding(.trailing, 10)
            .frame(height: 36)
            .background(palette.raised, in: RoundedRectangle(cornerRadius: Radius.row * 0.75))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .padding(.top, 8)
        .padding(.bottom, 4)
        .accessibilityLabel([row.name.map { "@\($0)" }, row.title, facts].compactMap { $0 }.joined(separator: ", "))
    }

    @ViewBuilder
    private func step(_ step: TurnStep, primary: Bool) -> some View {
        if let label = CallWords.step(step) {
            Chip(title: label, primary: primary, busy: taking == step) { take(step) }
        }
    }

    private func open() {
        guard let core = context.core else { return }
        Haptics.select()
        Router.shared.push(.chat(core: core, agent: row.agentId))
    }

    private func take(_ step: TurnStep) {
        guard taking == nil, let core = context.core, let connection = Hub.shared.live(core).connection else { return }
        taking = step
        Task {
            defer { taking = nil }
            do {
                _ = try await connection.call(PhoneCall.TakeStep.self, TakeStepParams(agentId: row.agentId, step: step))
                Haptics.success()
            } catch {
                Haptics.failure()
            }
        }
    }
}

extension ChatMessage {
    /// When it happened: when its turn ended, or when the person sent it.
    var time: Int? { endedAt ?? sentAt }
}

extension ResultRow {
    /// The workers' finished work that goes after each of `messages`, by its index: what finished after it and before
    /// the next message that has a time.
    static func placed(_ results: [ResultRow], in messages: [ChatMessage]) -> [Int: [ResultRow]] {
        guard !results.isEmpty else { return [:] }
        var placed: [Int: [ResultRow]] = [:]
        var next: Int?
        for index in messages.indices.reversed() {
            guard let at = messages[index].time else { continue }
            let found = results.filter { result in result.at >= at && next.map { result.at < $0 } ?? true }
            if !found.isEmpty { placed[index] = found }
            next = at
        }
        return placed
    }
}
