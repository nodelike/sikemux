import SwiftUI

/// How an agent's turn stands, as the Mac's rail and the notch mark a row, one state per row: its working squares; a
/// ring filling in the neutral ink as a CI or check wait with an estimate runs, with the minutes left; the pink timer
/// for any other wait, with the time left when it waits for a set time; the amber dot with how long a call has waited;
/// the purple dot once done until the person has seen it, red once failed, a hollow ring once stopped. The core says
/// which.
struct TurnMark: View {
    @Environment(\.palette) private var palette
    let turn: TurnState?
    let now: Date

    var body: some View {
        if let turn, turn.seen != true {
            HStack(spacing: 4) {
                if let minutes = CallWords.rowMinutes(turn, now: now) {
                    Text(minutes)
                        .font(Theme.ui(11))
                        .monospacedDigit()
                        .foregroundStyle(tint(turn))
                }
                mark(turn).frame(width: 16, height: 16)
            }
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(CallWords.rowLabel(turn, now: now))
        }
    }

    @ViewBuilder
    private func mark(_ turn: TurnState) -> some View {
        switch turn.state {
        case .working: Working()
        case .waiting:
            if let share = CallWords.ringShare(turn.wait, now: now) {
                WaitRing(share: share, fill: palette.secondary, track: palette.border).frame(width: 11, height: 11)
            } else {
                IconView(icon: Icons.timer, size: 11).foregroundStyle(palette.cmd)
            }
        case .needsYou: Dot(color: palette.warn, size: 7)
        case .done: Dot(color: palette.accent, size: 7)
        case .failed: Dot(color: palette.danger, size: 7)
        case .stopped: Dot(color: palette.secondary, size: 7, hollow: true)
        case .unknown: EmptyView()
        }
    }

    private func tint(_ turn: TurnState) -> Color {
        switch turn.state {
        case .needsYou: palette.warn
        case .waiting: CallWords.ringShare(turn.wait, now: now) == nil ? palette.cmd : palette.tertiary
        default: palette.tertiary
        }
    }
}

/// A ring that fills clockwise from the top as a wait runs, over its track.
struct WaitRing: View {
    let share: Double
    let fill: Color
    let track: Color

    var body: some View {
        ZStack {
            Circle().stroke(track, lineWidth: 1.6)
            Circle().trim(from: 0, to: share).stroke(fill, style: StrokeStyle(lineWidth: 1.6, lineCap: .round))
                .rotationEffect(.degrees(-90))
        }
    }
}

extension BuddyState {
    /// The face a turn wears: a finished turn the person has seen wears idle, as its row's mark does.
    init(_ turn: TurnState) {
        if turn.seen == true {
            self = .idle
            return
        }
        switch turn.state {
        case .working: self = .working
        case .needsYou: self = .waiting
        case .done: self = .done
        case .failed: self = .error
        default: self = .idle
        }
    }
}
