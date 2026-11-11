import SwiftUI

/// The end of a streamed answer still coming in, each piece at its own strength.
typealias Fading = [(text: String, opacity: Double)]

/// A streamed answer's newest words, drawn faint as they arrive and whole a moment later. The words are laid out once
/// per batch the core sends, never per frame: a batch draws its words faint, and the next batch, or one redraw once
/// the stream pauses, draws them whole. Kept by part, so a row the list reuses picks up where it was.
@MainActor
final class Reveal {
    /// How long a batch's words stay faint once the stream pauses.
    static let fade: TimeInterval = 0.18
    /// How strong a batch's words are while they are newest.
    private static let faint = 0.45

    private struct Seen {
        var length: Int
        var from: Int
        var at: Date
    }

    private var parts: [String: Seen] = [:]

    /// The newest words of `text` and when they came; text already there when it is first drawn shows whole.
    func newest(_ id: String, text: [UInt16], live: Bool) -> (fading: Fading, until: Date) {
        let total = text.count
        var seen = parts[id] ?? Seen(length: live ? 0 : total, from: total, at: .distantPast)
        if total != seen.length {
            seen.from = min(seen.length, total)
            seen.length = total
            seen.at = Date()
            parts[id] = seen
        }
        let until = seen.at.addingTimeInterval(Self.fade)
        let tail = text[seen.from...]
        guard seen.from < total, Self.fadesInline(tail) else { return ([], until) }
        return ([(String(decoding: tail, as: UTF16.self), Self.faint)], until)
    }

    /// Words still fading in can only be drawn faint where they sit inside a line of plain text.
    private static func fadesInline(_ text: ArraySlice<UInt16>) -> Bool {
        !text.contains { $0 == 10 || $0 == 96 || $0 == 124 }
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
