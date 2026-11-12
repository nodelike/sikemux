import SwiftUI

/// How an agent's turn stands, as the Mac's rail marks a row: its working squares, a ring filling as a wait runs with
/// the minutes beside it, the amber dot with how long a call has waited, the purple dot once done, red once failed,
/// hollow once stopped. One state per row; the core says which.
struct TurnMark: View {
    @Environment(\.palette) private var palette
    let turn: TurnState?
    let now: Date

    var body: some View {
        if let turn {
            HStack(spacing: 4) {
                if let minutes = CallWords.rowMinutes(turn, now: now) {
                    Text(minutes)
                        .font(Theme.ui(12, .medium))
                        .monospacedDigit()
                        .foregroundStyle(turn.state == .needsYou ? palette.warn : palette.secondary)
                }
                mark(turn)
            }
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(CallWords.rowLabel(turn, now: now))
        }
    }

    @ViewBuilder
    private func mark(_ turn: TurnState) -> some View {
        switch turn.state {
        case .working: Working()
        case .waiting: WaitRing(share: share(turn.wait), tint: palette.cmd).frame(width: 16, height: 16)
        case .needsYou: NeedsYou()
        case .done: Dot(color: palette.accent, size: 7).frame(width: 16, height: 16)
        case .failed: Dot(color: palette.danger, size: 7).frame(width: 16, height: 16)
        case .stopped: Dot(color: palette.secondary, size: 7, hollow: true).frame(width: 16, height: 16)
        case .unknown: EmptyView()
        }
    }

    /// How far a wait has gone: of its expected length, or of the time until its timer fires.
    private func share(_ wait: TurnWait?) -> Double {
        guard let wait else { return 0.25 }
        let nowMs = now.timeIntervalSince1970 * 1000
        let gone = nowMs - Double(wait.startedAt)
        let whole = wait.expectedMs.map(Double.init) ?? wait.until.map { Double($0 - wait.startedAt) }
        guard let whole, whole > 0 else { return 0.25 }
        return min(1, max(0.05, gone / whole))
    }
}

/// A ring that fills clockwise from the top as a wait runs.
struct WaitRing: View {
    let share: Double
    let tint: Color

    var body: some View {
        ZStack {
            Circle().stroke(tint.opacity(0.3), lineWidth: 1.6)
            Circle().trim(from: 0, to: share).stroke(tint, style: StrokeStyle(lineWidth: 1.6, lineCap: .round))
                .rotationEffect(.degrees(-90))
        }
        .padding(2.5)
    }
}

extension BuddyState {
    /// The face a turn's state wears.
    init(_ phase: TurnPhase?) {
        switch phase {
        case .working: self = .working
        case .needsYou: self = .waiting
        case .done: self = .done
        case .failed: self = .error
        default: self = .idle
        }
    }
}
