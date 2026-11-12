import SwiftUI

/// What happened while the person was away, at the end of the chief's chat, as the Mac draws it: the calls waiting,
/// which are the queue docked above the composer, what agents decided for them, with Overrule on a phone with full
/// access, and what landed, each row opening that agent's chat.
struct AwayCard: View {
    @Environment(\.palette) private var palette
    let core: String
    let away: ChiefAway
    let full: Bool

    var body: some View {
        let calls = away.calls ?? []
        let decided = away.decided ?? []
        let landed = away.landed ?? []
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                IconView(icon: Icons.logo, size: 15).foregroundStyle(palette.accent)
                Text(CallWords.whileAway).font(Theme.ui(14.5, .semibold)).foregroundStyle(palette.ink)
                if let back = away.back {
                    Text("· \(CallWords.awayFor(back - away.since))").font(Theme.ui(13)).foregroundStyle(
                        palette.tertiary)
                }
            }
            if !calls.isEmpty {
                section(CallWords.awayNeedsYou, calls.count)
                HStack(spacing: 8) {
                    Circle().fill(palette.warn).frame(width: 8, height: 8)
                    Text(CallWords.answerBelow(calls.count)).font(Theme.ui(14)).foregroundStyle(palette.secondary)
                }
                .frame(minHeight: 40)
            }
            if !decided.isEmpty {
                section(CallWords.decidedForYou, decided.count)
                ForEach(Array(decided.enumerated()), id: \.offset) { index, decision in
                    AwayDecisionRow(core: core, decision: decision, full: full)
                    if index < decided.count - 1 { Divider().overlay(palette.border) }
                }
            }
            if !landed.isEmpty {
                section(CallWords.landed, landed.count)
                ForEach(Array(landed.enumerated()), id: \.offset) { index, work in
                    Button {
                        Router.shared.push(.chat(core: core, agent: work.agentId))
                    } label: {
                        landedRow(work)
                    }
                    .buttonStyle(.plain)
                    if index < landed.count - 1 { Divider().overlay(palette.border) }
                }
            }
        }
        .padding(.horizontal, 12)
        .padding(.top, 12)
        .padding(.bottom, 6)
        .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(palette.overlay))
        .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).strokeBorder(palette.borderStrong))
        .padding(.top, 12)
        .padding(.bottom, 8)
    }

    private func section(_ label: String, _ count: Int) -> some View {
        HStack(spacing: 10) {
            Text("\(label) \(count)".uppercased()).font(Theme.ui(11, .medium)).tracking(2.2).foregroundStyle(
                palette.tertiary)
            Rectangle().fill(palette.border).frame(height: 1)
        }
        .padding(.top, 14)
        .padding(.bottom, 2)
    }

    private func landedRow(_ work: AwayLanded) -> some View {
        HStack(spacing: 10) {
            AskerMark(agentId: work.agentId, provider: provider(work.agentId), size: 16)
                .frame(width: 16)
                .frame(maxHeight: .infinity, alignment: .top)
                .padding(.top, 3)
            VStack(alignment: .leading, spacing: 2) {
                Text(work.title).font(Theme.ui(14.5)).foregroundStyle(palette.ink).lineLimit(2)
                Text(
                    [work.name.map { "@\($0)" }, Format.age(Date(timeIntervalSince1970: Double(work.at) / 1000))]
                        .compactMap { $0 }.joined(separator: " · ")
                )
                .font(Theme.ui(12.5)).foregroundStyle(palette.tertiary).lineLimit(1)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            IconView(icon: Icons.chevron, size: 12).foregroundStyle(palette.tertiary)
        }
        .padding(.vertical, 9)
        .frame(minHeight: 44)
        .contentShape(Rectangle())
    }

    private func provider(_ agentId: String) -> String {
        Hub.shared.live(core).view?.workspace.agents?.first { $0.id == agentId }?.provider ?? "agent"
    }
}

/// A decision made for the person, struck once they overrule it; Overrule only where the phone may act.
private struct AwayDecisionRow: View {
    @Environment(\.palette) private var palette
    let core: String
    let decision: AwayDecision
    let full: Bool
    @State private var sending = false
    @State private var refused: String?

    var body: some View {
        let struck = decision.overruled == true
        HStack(spacing: 10) {
            IconView(icon: Icons.check, size: 14)
                .foregroundStyle(palette.tertiary)
                .frame(width: 16)
                .frame(maxHeight: .infinity, alignment: .top)
                .padding(.top, 3)
            VStack(alignment: .leading, spacing: 2) {
                Text(decision.text)
                    .font(Theme.ui(14.5))
                    .strikethrough(struck)
                    .foregroundStyle(struck ? palette.tertiary : palette.ink)
                    .lineLimit(2)
                if let under = decision.why ?? decision.by.map({ "@\($0.name)" }) {
                    Text(under).font(Theme.ui(12.5)).foregroundStyle(palette.tertiary).lineLimit(1)
                }
                if let refused { Text(refused).font(Theme.ui(12.5)).foregroundStyle(palette.danger) }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            if full && !struck {
                Button(CallWords.overrule, action: overrule)
                    .buttonStyle(.plain)
                    .font(Theme.ui(13.5, .medium))
                    .foregroundStyle(palette.ink)
                    .padding(.horizontal, 12)
                    .frame(height: 32)
                    .overlay(RoundedRectangle(cornerRadius: 9).strokeBorder(palette.borderStrong))
                    .opacity(sending ? 0.5 : 1)
                    .disabled(sending)
            }
        }
        .padding(.vertical, 9)
        .frame(minHeight: 44)
    }

    private func overrule() {
        guard let connection = Hub.shared.live(core).connection, !sending else { return }
        sending = true
        refused = nil
        let params = OverruleParams(project: decision.project, goal: decision.goal, decision: decision.decision)
        Task {
            defer { sending = false }
            do {
                _ = try await connection.call(PhoneCall.OverruleDecision.self, params)
            } catch {
                refused = problem(error)
            }
        }
    }
}
