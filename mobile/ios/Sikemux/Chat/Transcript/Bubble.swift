import SwiftUI

extension Palette {
    /// The person's bubble: the accent warmed toward the ink, laid faintly over the sunken ground and opaque, as the
    /// Mac fills it, so a pane picture never shows through.
    var bubbleFill: Color { accent.mixed(with: ink, 0.25).mixed(with: sunken, 0.1) }
}

enum Bubble {
    static let radius: CGFloat = 16
    static let tail: CGFloat = 6

    static let mine = UnevenRoundedRectangle(
        topLeadingRadius: radius, bottomLeadingRadius: radius, bottomTrailingRadius: tail, topTrailingRadius: radius)
    static let theirs = UnevenRoundedRectangle(
        topLeadingRadius: radius, bottomLeadingRadius: tail, bottomTrailingRadius: radius, topTrailingRadius: radius)
}

extension View {
    /// The words of a message in a bubble, the person's or someone else's.
    func sentWords(_ palette: Palette) -> some View {
        font(Theme.ui(14)).lineSpacing(22 - 14 * 1.22).foregroundStyle(palette.ink)
    }

    /// The person's bubble: no border unless it failed to send, its tail at the bottom right.
    func mineBubble(_ palette: Palette, failed: Bool = false) -> some View {
        padding(.vertical, 10).padding(.horizontal, 14)
            .background(Bubble.mine.fill(palette.bubbleFill))
            .overlay { if failed { Bubble.mine.strokeBorder(palette.danger, lineWidth: 1) } }
    }

    /// What someone else said to the person: a bordered bubble with its tail at the bottom left.
    func theirBubble(_ palette: Palette) -> some View {
        padding(.vertical, 10).padding(.horizontal, 14)
            .background(Bubble.theirs.fill(palette.composer))
            .overlay(Bubble.theirs.strokeBorder(palette.border.opacity(0.8), lineWidth: 1))
    }

    /// At most `share` of the transcript's width, at the right like the person's own messages.
    func sentWidth(_ share: CGFloat = 0.84, alignment: Alignment = .trailing) -> some View {
        containerRelativeFrame(.horizontal, alignment: alignment) { width, _ in width * share }
    }
}

/// Who sent a bubble at the left, over it, with when at the line's end.
struct From<Who: View, Content: View>: View {
    @Environment(\.palette) private var palette
    var at: Int?
    @ViewBuilder let who: () -> Who
    @ViewBuilder let content: () -> Content

    var body: some View {
        HeadOverBubble(spacing: 6) {
            HStack(spacing: 8) {
                who()
                Spacer(minLength: 0)
                if let at {
                    Text(ChatWords.clock(at)).font(Theme.ui(11.5)).monospacedDigit().foregroundStyle(palette.inkFaint)
                        .padding(.leading, 8)
                }
            }
            .frame(minHeight: 22)
            content().theirBubble(palette)
        }
        .sentWidth(0.86, alignment: .leading)
    }
}

/// A line over a bubble as wide as the bubble, or as its own words when those are wider, so the time at its end sits
/// over the bubble's edge.
private struct HeadOverBubble: Layout {
    var spacing: CGFloat

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let (head, bubble) = sizes(proposal, subviews)
        return CGSize(width: max(head.width, bubble.width), height: head.height + spacing + bubble.height)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        let (head, bubble) = sizes(proposal, subviews)
        let width = max(head.width, bubble.width)
        subviews[0].place(
            at: bounds.origin, proposal: ProposedViewSize(width: width, height: head.height))
        subviews[1].place(
            at: CGPoint(x: bounds.minX, y: bounds.minY + head.height + spacing),
            proposal: ProposedViewSize(width: bubble.width, height: bubble.height))
    }

    private func sizes(_ proposal: ProposedViewSize, _ subviews: Subviews) -> (CGSize, CGSize) {
        let bubble = subviews[1].sizeThatFits(ProposedViewSize(width: proposal.width, height: nil))
        let ideal = subviews[0].sizeThatFits(.unspecified)
        let head = CGSize(width: min(ideal.width, proposal.width ?? ideal.width), height: ideal.height)
        return (head, bubble)
    }
}
