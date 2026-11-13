import SwiftUI

/// An agent at work: the rail's three-by-three twinkling squares.
struct Working: View {
    @Environment(\.palette) private var palette
    @Environment(\.accessibilityReduceMotion) private var still
    private static let periods: [Double] = [1.2, 1.58, 0.9, 1.4, 1.05, 1.3, 0.95, 1.5, 1.15]

    var body: some View {
        TimelineView(.animation(minimumInterval: 1 / 30, paused: still)) { timeline in
            let time = timeline.date.timeIntervalSinceReferenceDate
            Canvas { context, _ in
                for (index, period) in Self.periods.enumerated() {
                    let rect = CGRect(x: Double(index % 3) * 5, y: Double(index / 3) * 5, width: 3, height: 3)
                    context.opacity =
                        still
                        ? 1 : Self.opacity(time, period: period, delay: Double(index * 230 % Int(period * 1000)) / 1000)
                    context.fill(Path(roundedRect: rect, cornerRadius: 0.6), with: .color(palette.live))
                }
            }
        }
        .frame(width: 13, height: 13)
        .accessibilityElement()
        .accessibilityLabel("Working")
    }

    /// 0.2 at the ends of each period and 1 at its middle, eased in and out.
    private static func opacity(_ time: Double, period: Double, delay: Double) -> Double {
        let phase = (time - delay).truncatingRemainder(dividingBy: period) / period
        let rise = phase < 0.5 ? phase * 2 : (1 - phase) * 2
        let eased = rise < 0.5 ? 2 * rise * rise : 1 - pow(-2 * rise + 2, 2) / 2
        return 0.2 + 0.8 * eased
    }
}

/// Something is waiting on the person: the Mac rail's amber dot, sending out two rings as it appears.
struct NeedsYou: View {
    @Environment(\.palette) private var palette
    @Environment(\.accessibilityReduceMotion) private var still
    @State private var rung = false

    var body: some View {
        ZStack {
            if !still {
                ForEach(0..<2, id: \.self) { ring in
                    Circle()
                        .strokeBorder(palette.warn, lineWidth: 1)
                        .frame(width: 7, height: 7)
                        .scaleEffect(rung ? 2.4 : 0.6)
                        .opacity(rung ? 0 : 0.9)
                        .animation(
                            .timingCurve(0.2, 0.6, 0.3, 1, duration: 0.52).delay(Double(ring) * 0.38), value: rung)
                }
            }
            Circle().fill(palette.warn).frame(width: 7, height: 7)
        }
        .frame(width: 16, height: 16)
        .onAppear { rung = true }
        .accessibilityElement()
        .accessibilityLabel("Needs input")
    }
}

/// A count beside a small glyph at a row's end, as the Mac's rail counts subagents, shells and trigger times.
struct MarkCount: View {
    @Environment(\.palette) private var palette
    let icon: IconDef
    let tint: Color
    let text: String
    var dim = false

    var body: some View {
        HStack(spacing: 3) {
            IconView(icon: icon, size: 11).foregroundStyle(tint)
            Text(text)
                .font(Theme.ui(12, .medium))
                .monospacedDigit()
                .foregroundStyle(dim ? palette.tertiary : palette.secondary)
        }
        .accessibilityElement(children: .ignore)
    }
}

/// The subagents an agent still has running, as the Mac's tabs and rail count them.
struct SubagentCount: View {
    @Environment(\.palette) private var palette
    let count: Int

    var body: some View {
        MarkCount(icon: Icons.agent, tint: palette.live, text: "\(count)")
            .accessibilityLabel(
                count == 1 ? String(localized: "1 subagent running") : String(localized: "\(count) subagents running"))
    }
}

/// The shells and monitors an agent still has running, counted beside the Mac's terminal glyph.
struct ShellCount: View {
    @Environment(\.palette) private var palette
    let count: Int

    var body: some View {
        MarkCount(icon: Icons.command, tint: palette.warn, text: "\(count)")
            .accessibilityLabel(
                count == 1
                    ? String(localized: "1 shell or monitor running")
                    : String(localized: "\(count) shells or monitors running"))
    }
}

/// Where a heavy command the agent started stands in the host's line, as the Mac's rail says it: "2nd in line".
struct LinePlace: View {
    @Environment(\.palette) private var palette
    let place: Int

    var body: some View {
        MarkCount(icon: Icons.hourglass, tint: palette.warn, text: CallWords.inLine(place))
            .accessibilityLabel(CallWords.inLine(place))
    }
}

/// How soon a snoozed agent wakes, as the Mac rail says it: minutes, hours, then the weekday.
struct WakeTime: View {
    @Environment(\.palette) private var palette
    let at: Date
    let now: Date

    var body: some View {
        let left = max(0, at.timeIntervalSince(now))
        let words =
            left < 3600
            ? String(localized: "\(max(1, Int((left / 60).rounded(.up))))m")
            : left < 86_400
                ? String(localized: "\(max(1, Int((left / 3600).rounded())))h")
                : at.formatted(.dateTime.weekday(.abbreviated))
        MarkCount(icon: Icons.clock, tint: palette.tertiary, text: words, dim: true)
            .accessibilityLabel(String(localized: "Snoozed, wakes in \(words)"))
    }
}

struct Dot: View {
    let color: Color
    var size: CGFloat = 8
    var hollow = false

    var body: some View {
        Group {
            if hollow {
                Circle().strokeBorder(color, lineWidth: 1.5)
            } else {
                Circle().fill(color)
            }
        }
        .frame(width: size, height: size)
    }
}

/// The minute a screen last ticked over, so ages and countdowns move once a minute and nothing else redraws.
struct MinuteClock<Content: View>: View {
    @ViewBuilder var content: (Date) -> Content

    var body: some View {
        TimelineView(.everyMinute) { timeline in content(timeline.date) }
    }
}
