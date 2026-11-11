import SwiftUI

/// The end of a streamed answer still coming in, each piece at its own strength.
typealias Fading = [(text: String, opacity: Double)]

/// Streamed words come out at a steady pace however the network delivers them, each fading in as it appears; text
/// already there when it is drawn shows whole. Kept by part, so a row the list reuses picks up where it was.
@MainActor
final class Reveal {
    /// The pace words come out at, in characters a millisecond, faster than anyone reads but never a jump.
    private static let pace = 0.9
    /// How far behind the stream the words may fall before they speed up to keep within it.
    private static let maxBehind = 1500.0
    /// How long a word takes to fade in, in milliseconds.
    private static let fade = 180.0
    /// How far past the pace a step may run to finish the word it stopped in.
    private static let wordReach = 16

    private struct Step {
        var start: Int
        var end: Int
        var at: Double
    }

    private struct Shown {
        var length: Int
        var carry = 0.0
        var steps: [Step] = []
        var last = 0.0
    }

    private var parts: [String: Shown] = [:]

    /// Whether `id` is still revealing words, so its drawing must tick.
    func moving(_ id: String, total: Int, live: Bool) -> Bool {
        guard let shown = parts[id] else { return live }
        return shown.length < total || !shown.steps.isEmpty
    }

    /// How much of `text` shows at `now`, and its end still fading in.
    func step(_ id: String, text: [UInt16], live: Bool, now: Date) -> (length: Int, fading: Fading) {
        let total = text.count
        var shown = parts[id] ?? Shown(length: live ? 0 : total)
        let ms = now.timeIntervalSinceReferenceDate * 1000
        let since = shown.last > 0 ? min(ms - shown.last, 50) : 16
        shown.last = ms
        let from = min(shown.length, total)
        let behind = total - from
        let carry = behind > 0 ? shown.carry + max(Self.pace, Double(behind) / Self.maxBehind) * since : 0
        let end = behind > 0 ? Self.wordEnd(text, from + Int(carry)) : from
        shown.steps = shown.steps.filter { ms - $0.at < Self.fade && $0.end <= end }
        if end > from { shown.steps.append(Step(start: from, end: end, at: ms)) }
        shown.carry = max(0, carry - Double(end - from))
        shown.length = end
        parts[id] = shown
        let revealed = Array(text[..<end])
        guard let first = shown.steps.first, Self.fadesInline(revealed[first.start...]) else { return (end, []) }
        let fading = shown.steps.map { step in
            (String(decoding: revealed[step.start..<step.end], as: UTF16.self), min(1, (ms - step.at) / Self.fade))
        }
        return (end, fading)
    }

    private static func wordEnd(_ text: [UInt16], _ at: Int) -> Int {
        guard at < text.count else { return text.count }
        for end in at..<min(text.count, at + wordReach) where text[end] == 32 || text[end] == 10 || text[end] == 9 { return end }
        return at
    }

    /// Words still fading in can only be drawn faint where they sit inside a line of plain text.
    private static func fadesInline(_ text: ArraySlice<UInt16>) -> Bool {
        !text.contains { $0 == 10 || $0 == 96 || $0 == 124 }
    }
}

/// The blocks as far as their first `chars` visible characters.
func revealBlocks(_ blocks: [MdElement], _ chars: Int) -> [MdElement] {
    cutNodes(blocks.map(MdNode.element), chars).compactMap { node in
        if case .element(let element) = node { return element }
        return nil
    }
}

private func visibleLength(_ node: MdNode) -> Int {
    switch node {
    case .text(let text): return text.utf16.count
    case .element(let element):
        switch element {
        case .pre(let pre): return pre.v.utf16.count
        case .code(let code): return code.v.utf16.count
        case .br: return 1
        case .table(let table): return ((table.head ?? []) + table.rows.flatMap { $0 }).flatMap { $0 }.map(visibleLength).reduce(0, +)
        default: return (element.children ?? []).map(visibleLength).reduce(0, +)
        }
    }
}

private func cutNodes(_ nodes: [MdNode], _ chars: Int) -> [MdNode] {
    var kept: [MdNode] = []
    var left = chars
    for node in nodes {
        guard left > 0 else { break }
        let length = visibleLength(node)
        kept.append(length <= left ? node : cutNode(node, left))
        left -= length
    }
    return kept
}

private func prefix(_ text: String, _ units: Int) -> String {
    String(decoding: Array(text.utf16.prefix(units)), as: UTF16.self)
}

private func cutNode(_ node: MdNode, _ chars: Int) -> MdNode {
    switch node {
    case .text(let text): return .text(prefix(text, chars))
    case .element(var element):
        switch element {
        case .pre(var pre):
            pre.v = prefix(pre.v, chars)
            return .element(.pre(pre))
        case .code(var code):
            code.v = prefix(code.v, chars)
            return .element(.code(code))
        default:
            if let children = element.children { element.children = cutNodes(children, chars) }
            return .element(element)
        }
    }
}

extension AttributedString {
    /// These words with their end, still coming in, drawn fainter piece by piece.
    func fading(_ fading: Fading, color: Color) -> AttributedString {
        let coming = fading.map(\.text).joined()
        let trimmed = coming.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, String(characters).hasSuffix(coming) else { return self }
        var out = self
        var cursor = out.endIndex
        for piece in fading.reversed() {
            guard let start = out.characters.index(cursor, offsetBy: -piece.text.count, limitedBy: out.startIndex) else { break }
            out[start..<cursor].foregroundColor = color.opacity(piece.opacity)
            cursor = start
        }
        return out
    }
}

/// The answer being written opens up to each new line rather than stepping, so what is drawn under it holds still.
struct Grow<Content: View>: View {
    @Environment(\.accessibilityReduceMotion) private var still
    let live: Bool
    @ViewBuilder let content: Content
    @State private var height: CGFloat?

    var body: some View {
        let growing = live && !still
        content
            .fixedSize(horizontal: false, vertical: true)
            .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { next in
                if growing, let shown = height, next > shown {
                    withAnimation(.easeOut(duration: 0.22)) { height = next }
                } else {
                    height = next
                }
            }
            .frame(height: growing ? height : nil, alignment: .top)
            .clipped()
    }
}
