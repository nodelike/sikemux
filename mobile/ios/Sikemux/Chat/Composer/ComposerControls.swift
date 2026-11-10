import SwiftUI

/// YOLO or safe, as the Mac's toggle says it; a tap switches it where the host can.
struct YoloToggle: View {
    @Environment(\.palette) private var palette
    /// The host the agent runs on.
    let core: String
    let mode: String
    let provider: String
    /// A turn is running or something waits on an answer, and the host refuses a switch then.
    let locked: Bool
    let onToggle: (String) -> Void

    private var on: Bool { mode == "bypass" }

    var body: some View {
        if Catalog.canSwitchMode(provider, core: core) {
            Button {
                Haptics.select()
                onToggle(on ? "workspace-write" : "bypass")
            } label: {
                look
            }
            .buttonStyle(ActiveFill(radius: 7))
            .disabled(locked)
            .opacity(locked ? 0.55 : 1)
            .accessibilityLabel("Run without asking")
            .accessibilityValue(on ? "On" : "Off")
            .accessibilityAddTraits(.isToggle)
            .accessibilityHint(locked ? "Changes once the turn ends" : "")
        } else {
            look
                .accessibilityElement(children: .ignore)
                .accessibilityLabel(on ? "Runs without asking" : "Asks before acting")
        }
    }

    private var look: some View {
        HStack(spacing: 4) {
            IconView(icon: on ? Icons.shieldBolt : Icons.shield, size: 13)
                .foregroundStyle(on ? palette.accent : palette.inkFaint)
            Text(on ? "YOLO" : "SAFE")
                .font(Theme.ui(11, .semibold))
                .tracking(0.9)
                .foregroundStyle(on ? AnyShapeStyle(yolo) : AnyShapeStyle(palette.inkFaint))
        }
        .padding(.horizontal, 8)
        .frame(height: 34)
        .contentShape(Rectangle())
    }

    /// Twice the word's width, as the Mac draws it, so the word shows the first half.
    private var yolo: LinearGradient {
        LinearGradient(
            colors: [palette.accent, palette.cmd, palette.danger, palette.warn, palette.live, palette.accent],
            startPoint: .leading, endPoint: UnitPoint(x: 2, y: 0.5)
        )
    }
}

/// A control that lights the active fill while pressed, as the composer's bar buttons do.
struct ActiveFill: ButtonStyle {
    @Environment(\.palette) private var palette
    var radius: CGFloat = 17

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .background(RoundedRectangle(cornerRadius: radius, style: .continuous).fill(configuration.isPressed ? palette.active : .clear))
    }
}

/// How much of the context window is used, from 0 to 1, and the Mac's colour for it: the agent's own, then warm
/// from 70%, then hot from 90%.
enum ContextFill {
    static func share(_ usage: ContextUsage) -> Double {
        usage.size > 0 ? min(1, max(0, usage.used / usage.size)) : 0
    }

    @MainActor
    static func color(_ usage: ContextUsage, provider: String, palette: Palette) -> Color {
        let share = share(usage)
        if share >= 0.9 { return palette.danger }
        if share >= 0.7 { return palette.warn }
        if Catalog.readsUsage(provider), let brand = Catalog.brand(provider) { return brand }
        return palette.accent
    }

    static func tokens(_ count: Double) -> String {
        count.formatted(.number.notation(.compactName).precision(.fractionLength(0...1)))
    }

    static func money(_ cost: UsageCost) -> String {
        cost.amount.formatted(.currency(code: cost.currency).precision(.fractionLength(0...2)))
    }
}

/// How much of the context window the chat has used, as the Mac's composer rings it. A tap opens the settings sheet.
struct ContextRing: View {
    @Environment(\.palette) private var palette
    let usage: ContextUsage
    let provider: String
    let onPress: () -> Void

    private static let ring: CGFloat = 14
    private static let stroke: CGFloat = 1.8

    var body: some View {
        let share = ContextFill.share(usage)
        Button(action: onPress) {
            ZStack {
                Circle().stroke(palette.border, lineWidth: Self.stroke)
                Circle()
                    .trim(from: 0, to: share)
                    .stroke(ContextFill.color(usage, provider: provider, palette: palette), style: StrokeStyle(lineWidth: Self.stroke, lineCap: .round))
                    .rotationEffect(.degrees(-90))
            }
            .frame(width: Self.ring - Self.stroke, height: Self.ring - Self.stroke)
            .frame(width: 34, height: 34)
            .contentShape(Circle())
        }
        .buttonStyle(ActiveFill())
        .accessibilityLabel("Context window \(Int((share * 100).rounded()))% used")
    }
}

/// Whether the first message starts the chat in a new git worktree on its own branch, beside the project it works in.
struct WorktreeSwitch: View {
    @Environment(\.palette) private var palette
    let on: Bool
    let disabled: Bool
    let onFlip: () -> Void

    var body: some View {
        Button {
            Haptics.select()
            onFlip()
        } label: {
            HStack(spacing: 5) {
                IconView(icon: Icons.git, size: 12).foregroundStyle(on ? palette.accent : palette.tertiary)
                Text("worktree").font(Theme.ui(12)).foregroundStyle(on ? palette.ink : palette.tertiary)
            }
            .padding(.horizontal, 8)
            .frame(height: 26)
            .contentShape(Rectangle())
        }
        .buttonStyle(ActiveFill(radius: 13))
        .disabled(disabled)
        .opacity(disabled ? 0.55 : 1)
        .accessibilityLabel("Worktree")
        .accessibilityHint("Starts this chat in a new git worktree on its own branch")
        .accessibilityValue(on ? "On" : "Off")
        .accessibilityAddTraits(.isToggle)
    }
}

/// The line above a chat's first message saying which worktree it works in: its branch, then its folder more quietly.
struct WorktreeNote: View {
    @Environment(\.palette) private var palette
    let branch: String
    let path: String

    var body: some View {
        HStack(spacing: 10) {
            IconView(icon: Icons.git, size: 14)
                .foregroundStyle(palette.accent)
                .frame(width: 30, height: 30)
                .background(RoundedRectangle(cornerRadius: 7, style: .continuous).fill(palette.accent.opacity(0.16)))
            VStack(alignment: .leading, spacing: 2) {
                (Text("Working in a worktree on ").font(Theme.ui(12)).foregroundColor(palette.tertiary)
                    + Text(branch).font(Theme.mono(12)).foregroundColor(palette.ink))
                    .lineLimit(1)
                Text(path)
                    .font(Theme.mono(11.5))
                    .foregroundStyle(palette.inkFaint)
                    .lineLimit(1)
                    .truncationMode(.head)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(.top, 12)
        .padding(.bottom, 4)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Working in a worktree on \(branch), at \(path)")
    }
}

/// An issue, pull request or other item handed over with a message, by its number and title rather than its whole
/// text.
struct ContextChips: View {
    @Environment(\.palette) private var palette
    let items: [SentContext]

    var body: some View {
        FlowRight(spacing: 6) {
            ForEach(items, id: \.uri) { item in
                let tracked = item.tracked
                let issue = tracked?.kind == .issue
                let label = tracked == nil ? "Context" : issue ? "Issue" : "Pull request"
                HStack(spacing: 6) {
                    IconView(icon: tracked == nil ? Icons.file : issue ? Icons.issue : Icons.pullRequest, size: 14)
                        .foregroundStyle(tracked == nil ? palette.inkFaint : palette.live)
                    if let number = tracked?.number {
                        Text("#\(number)").font(Theme.ui(12, .medium)).foregroundStyle(palette.ink)
                    }
                    Text(tracked?.title ?? item.title).font(Theme.ui(12)).foregroundStyle(palette.inkDim).lineLimit(1)
                }
                .padding(.vertical, 7)
                .padding(.horizontal, 10)
                .frame(maxWidth: 240)
                .background(RoundedRectangle(cornerRadius: 9, style: .continuous).fill(palette.raised))
                .overlay(RoundedRectangle(cornerRadius: 9, style: .continuous).strokeBorder(palette.border, lineWidth: 1))
                .accessibilityElement(children: .ignore)
                .accessibilityLabel("\(label) \(tracked.map { "#\($0.number) " } ?? "")\(tracked?.title ?? item.title)")
            }
        }
        .padding(.bottom, 6)
    }
}

/// Lays its children in rows from the right edge, wrapping when a row is full, as a sent message's chips sit.
struct FlowRight: Layout {
    var spacing: CGFloat

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let rows = rows(proposal.width ?? .infinity, subviews)
        let width = rows.map(\.width).max() ?? 0
        let height = rows.map(\.height).reduce(0, +) + spacing * CGFloat(max(0, rows.count - 1))
        return CGSize(width: width, height: height)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        var y = bounds.minY
        for row in rows(bounds.width, subviews) {
            var x = bounds.maxX - row.width
            for index in row.items {
                let size = subviews[index].sizeThatFits(.unspecified)
                subviews[index].place(at: CGPoint(x: x, y: y), proposal: ProposedViewSize(size))
                x += size.width + spacing
            }
            y += row.height + spacing
        }
    }

    private func rows(_ limit: CGFloat, _ subviews: Subviews) -> [(items: [Int], width: CGFloat, height: CGFloat)] {
        var rows: [(items: [Int], width: CGFloat, height: CGFloat)] = []
        for (index, subview) in subviews.enumerated() {
            let size = subview.sizeThatFits(.unspecified)
            if let last = rows.last, last.width + spacing + size.width <= limit {
                rows[rows.count - 1].items.append(index)
                rows[rows.count - 1].width += spacing + size.width
                rows[rows.count - 1].height = max(last.height, size.height)
            } else {
                rows.append(([index], min(size.width, limit), size.height))
            }
        }
        return rows
    }
}
