import AppKit
import SwiftUI

/// The rail's state marks: a twinkling grid while working, an amber dot that
/// rings twice when it starts needing you, a purple dot when done and unseen.
struct StateMark: View {
    let state: AgentState

    var body: some View {
        switch state {
        case .working: TwinkleGrid(colour: Theme.live).frame(width: 16, height: 16)
        case .blocked: PingDot(colour: Theme.warn)
        case .done: Dot(colour: Theme.accent)
        case .idle: EmptyView()
        }
    }
}

/// Shells or monitors an agent left running, in the terminal glyph so it does not read as the dot that waits on you.
struct StillRunning: View {
    var body: some View {
        IconView(icon: Icons.command, size: 11)
            .foregroundStyle(Theme.warn)
            .frame(width: 16, height: 16)
            .accessibilityLabel("Shells or monitors still running")
    }
}

struct SubagentCount: View {
    let count: Int

    var body: some View {
        HStack(spacing: 3) {
            IconView(icon: Icons.agent, size: 10).foregroundStyle(Theme.live.opacity(0.75))
            Text("\(count)").font(Theme.ui(11, .medium)).monospacedDigit().foregroundStyle(Theme.inkFaint)
        }
        .fixedSize()
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(count) \(count == 1 ? "subagent" : "subagents") running")
    }
}

private struct Dot: View {
    let colour: Color

    var body: some View {
        Circle().fill(colour).frame(width: 7, height: 7).frame(width: 16, height: 16)
    }
}

private struct PingDot: View {
    let colour: Color
    @State private var rang = false

    var body: some View {
        ZStack {
            ForEach(0..<2, id: \.self) { ring in
                Circle()
                    .stroke(colour, lineWidth: 1.5)
                    .frame(width: 7, height: 7)
                    .scaleEffect(rang ? 2.4 : 0.6)
                    .opacity(rang ? 0 : 0.9)
                    .animation(.timingCurve(0.2, 0.6, 0.3, 1, duration: 0.52).delay(Double(ring) * 0.38), value: rang)
            }
            Circle().fill(colour).frame(width: 7, height: 7)
        }
        .frame(width: 16, height: 16)
        .onAppear { rang = true }
    }
}

private struct TwinkleGrid: NSViewRepresentable {
    let colour: Color

    func makeNSView(context: Context) -> TwinkleGridView { TwinkleGridView() }

    func updateNSView(_ view: TwinkleGridView, context: Context) {
        view.colour = NSColor(colour).cgColor
    }

    func sizeThatFits(_ proposal: ProposedViewSize, nsView: TwinkleGridView, context: Context) -> CGSize? {
        CGSize(width: TwinkleGridView.side, height: TwinkleGridView.side)
    }
}

/// Core Animation runs the twinkle so SwiftUI does no work per frame.
final class TwinkleGridView: NSView {
    static let side: CGFloat = 16
    private static let cellSide: CGFloat = 2.5
    private static let spacing: CGFloat = 1.5
    private static let cells: [(period: Double, offset: Double)] = [
        (0.9, 0), (1.24, -0.52), (1.58, -1.04), (1.07, -0.39), (1.41, -0.91),
        (0.9, -0.26), (1.24, -0.78), (1.58, -0.13), (1.07, -0.65),
    ]
    private static let samples = 100
    private static let keyTimes = (0...samples).map { NSNumber(value: Double($0) / Double(samples)) }
    private static let values = (0...samples).map { opacity(at: Double($0) / Double(samples)) }

    private let cellLayers: [CALayer] = cells.map { _ in CALayer() }

    var colour: CGColor = NSColor.white.cgColor {
        didSet { cellLayers.forEach { $0.backgroundColor = colour } }
    }

    init() {
        super.init(frame: CGRect(x: 0, y: 0, width: Self.side, height: Self.side))
        wantsLayer = true
        for cell in cellLayers {
            cell.cornerRadius = 0.5
            cell.backgroundColor = colour
            layer?.addSublayer(cell)
        }
        startTwinkling()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override var intrinsicContentSize: NSSize { NSSize(width: Self.side, height: Self.side) }

    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    override func layout() {
        super.layout()
        let grid = Self.cellSide * 3 + Self.spacing * 2
        let inset = (Self.side - grid) / 2
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        for (index, cell) in cellLayers.enumerated() {
            let top = inset + CGFloat(index / 3) * (Self.cellSide + Self.spacing)
            let rect = CGRect(
                x: inset + CGFloat(index % 3) * (Self.cellSide + Self.spacing),
                y: isFlipped ? top : bounds.height - top - Self.cellSide,
                width: Self.cellSide,
                height: Self.cellSide
            )
            cell.frame = backingAlignedRect(rect, options: .alignAllEdgesNearest)
        }
        CATransaction.commit()
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        if window != nil { startTwinkling() }
    }

    override func viewDidChangeBackingProperties() {
        super.viewDidChangeBackingProperties()
        let scale = window?.backingScaleFactor ?? 2
        cellLayers.forEach { $0.contentsScale = scale }
        needsLayout = true
    }

    private func startTwinkling() {
        let now = Date().timeIntervalSinceReferenceDate
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        for (layer, cell) in zip(cellLayers, Self.cells) {
            let phase = Self.phase(now, cell)
            layer.opacity = Float(Self.opacity(at: phase))
            let twinkle = CAKeyframeAnimation(keyPath: "opacity")
            twinkle.values = Self.values
            twinkle.keyTimes = Self.keyTimes
            twinkle.calculationMode = .linear
            twinkle.duration = cell.period
            twinkle.repeatCount = .infinity
            twinkle.timeOffset = phase * cell.period
            twinkle.isRemovedOnCompletion = false
            layer.add(twinkle, forKey: "twinkle")
        }
        CATransaction.commit()
    }

    private static func phase(_ time: Double, _ cell: (period: Double, offset: Double)) -> Double {
        let phase = ((time + cell.offset) / cell.period).truncatingRemainder(dividingBy: 1)
        return phase < 0 ? phase + 1 : phase
    }

    /// The app's keyframes: dim, brightest two fifths of the way through, dim again.
    private static func opacity(at phase: Double) -> Double {
        let rise = phase < 0.4 ? phase / 0.4 : 1 - (phase - 0.4) / 0.6
        let eased = rise * rise * (3 - 2 * rise)
        return 0.2 + 0.8 * eased
    }
}

struct CapsuleButton: View {
    let title: String
    var primary = false
    var height: CGFloat = 32
    let action: () -> Void

    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            Text(title)
                .font(Theme.ui(12.5, .semibold))
                .foregroundStyle(primary ? Theme.onAccent : Theme.ink)
                .frame(maxWidth: .infinity)
                .frame(height: height)
                .background(Capsule().fill(fill))
                .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
    }

    private var fill: Color {
        if primary { return hovering ? Color(hex: "#b08fff") : Theme.accent }
        return Color.white.opacity(hovering ? 0.16 : 0.1)
    }
}

/// A capsule that fills on hover, for icons and chips in the island.
struct HoverCapsule<Label: View>: View {
    var selected = false
    var selectedFill = Color.white.opacity(0.12)
    let action: () -> Void
    @ViewBuilder let label: () -> Label

    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            label().background(Capsule().fill(selected ? selectedFill : hovering ? Theme.hover : .clear)).contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
    }
}

/// How long an agent has been in its state: a clock while it works or waits,
/// how long ago once it finished.
struct SinceText: View {
    let state: AgentState
    let since: Date?

    var body: some View {
        TimelineView(.periodic(from: .now, by: 1)) { context in
            Text(Self.format(state, since, context.date))
                .font(Theme.ui(11.5))
                .monospacedDigit()
                .foregroundStyle(Theme.inkFaint)
        }
    }

    static func format(_ state: AgentState, _ since: Date?, _ now: Date) -> String {
        guard let since, state != .idle else { return "" }
        let seconds = max(0, Int(now.timeIntervalSince(since)))
        if state == .done {
            if seconds < 60 { return "just now" }
            if seconds < 3600 { return "\(seconds / 60)m ago" }
            return "\(seconds / 3600)h ago"
        }
        let hours = seconds / 3600
        let minutes = seconds % 3600 / 60
        let rest = seconds % 60
        return hours > 0 ? String(format: "%d:%02d:%02d", hours, minutes, rest) : String(format: "%d:%02d", minutes, rest)
    }
}
