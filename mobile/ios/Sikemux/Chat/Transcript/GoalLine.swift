import SwiftUI

/// Where a goal starts, takes a step, ends or stops, as a milestone: its mark and state in its colour, its name, then
/// its steps and time.
struct GoalLine: View {
    @Environment(\.palette) private var palette
    let notice: AgentNotice
    let goal: GoalMarker

    var body: some View {
        let (state, word) = said
        let name = goal.name?.nilIfEmpty ?? notice.title
        let progress = self.progress
        HStack(spacing: 9) {
            GoalStateMark(state: state)
            Text(word).font(Theme.ui(12.5, .semibold)).foregroundStyle(state.ink(palette)).fixedSize()
            Text(name).font(Theme.ui(12.5)).foregroundStyle(palette.secondary).lineLimit(1)
            Spacer(minLength: 0)
            HStack(spacing: 8) {
                GoalSteps(steps: goal.steps ?? [])
                if let progress { Text(progress).font(Theme.ui(12.5)).foregroundStyle(palette.tertiary) }
                if let took = goal.took, took >= 1000 {
                    Text(ChatWords.duration(ms: Double(took))).font(Theme.ui(11.5)).monospacedDigit()
                        .foregroundStyle(palette.inkFaint)
                }
            }
            .fixedSize()
        }
        .frame(minHeight: 36)
        .padding(.vertical, 6)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel([word, name, progress].compactMap { $0 }.joined(separator: ", "))
    }

    /// What the line says and the state its mark takes, from the moment of the goal it marks.
    private var said: (SliceState, String) {
        switch goal.moment ?? .started {
        case .step: (.done, String(localized: "Step done"))
        case .done: (.done, String(localized: "Goal done"))
        case .stopped: (goal.state == .failed ? .failed : .waiting, String(localized: "Goal stopped"))
        default:
            switch goal.state {
            case .needsYou: (.needsYou, String(localized: "Goal needs you"))
            case .waiting: (.waiting, String(localized: "Goal waiting"))
            default: (goal.state, String(localized: "Goal"))
            }
        }
    }

    /// How far a goal's steps have come: "2 of 5 done", or of its gates.
    private var progress: String? {
        guard let steps = goal.steps, !steps.isEmpty else { return nil }
        let done = steps.filter { $0 == .done }.count
        return goal.ofGates == true
            ? String(localized: "\(done) of \(steps.count) gates") : String(localized: "\(done) of \(steps.count) done")
    }
}

extension SliceState {
    func ink(_ palette: Palette) -> Color {
        switch self {
        case .done, .moving: palette.live
        case .needsYou: palette.warn
        case .failed: palette.danger
        default: palette.tertiary
        }
    }
}

/// A goal's steps as a strip of segments, one per step.
struct GoalSteps: View {
    @Environment(\.palette) private var palette
    let steps: [SliceState]

    private static let step: CGFloat = 14
    private static let gap: CGFloat = 3
    private static let most: CGFloat = 96

    var body: some View {
        if !steps.isEmpty {
            let count = CGFloat(steps.count)
            HStack(spacing: Self.gap) {
                ForEach(Array(steps.enumerated()), id: \.offset) { _, step in
                    segment(step).frame(maxWidth: .infinity).frame(height: 4)
                }
            }
            .frame(width: min(Self.most, count * Self.step + (count - 1) * Self.gap))
        }
    }

    @ViewBuilder private func segment(_ state: SliceState) -> some View {
        let shape = RoundedRectangle(cornerRadius: 2)
        switch state {
        case .done: shape.fill(palette.live)
        case .moving: shape.fill(palette.live.opacity(0.45))
        case .needsYou: shape.fill(palette.warn)
        case .failed: shape.fill(palette.danger)
        default: shape.strokeBorder(palette.inkFaint, style: StrokeStyle(lineWidth: 1, dash: [2, 2]))
        }
    }
}

/// A goal's mark: solid green done, a green ring moving, a dashed grey ring waiting, amber needing you, red stopped.
struct GoalStateMark: View {
    @Environment(\.palette) private var palette
    let state: SliceState

    var body: some View {
        let ink = state.ink(palette)
        Group {
            switch state {
            case .moving: Circle().strokeBorder(ink, lineWidth: 1.5)
            case .waiting: Circle().strokeBorder(ink, style: StrokeStyle(lineWidth: 1, dash: [2, 1.5]))
            default: Circle().fill(ink)
            }
        }
        .frame(width: 8, height: 8)
        .frame(width: 14, height: 14)
    }
}
