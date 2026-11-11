import SwiftUI

/// The places the find bar found in one part, as UTF-16 offsets in the words its tree shows, and where the run being
/// drawn starts among them.
struct Marks: Equatable {
    var ranges: [Range<Int>]
    /// The start of the place the reader is on, when it is in this part.
    var current: Int?
    var base = 0

    func from(_ start: Int) -> Marks {
        Marks(ranges: ranges, current: current, base: start)
    }
}

/// Counts the words a run has drawn so far, so each place found is marked where it stands.
final class FindCursor {
    var at: Int
    init(_ at: Int) { self.at = at }
}

/// The host counts a part's words as its tree shows them: top-level blocks and the entries of lists, quotes and folds
/// a line apart, code by its text, pictures and rules as nothing.
enum FindText {
    static func length(_ node: MdNode) -> Int {
        switch node {
        case .text(let text): return text.utf16.count
        case .element(let element): return length(element)
        }
    }

    static func length(_ element: MdElement) -> Int {
        switch element {
        case .code(let code): return code.v.utf16.count
        case .pre(let pre): return pre.v.utf16.count
        case .br: return 1
        case .img, .hr, .fnref: return 0
        case .table(let table):
            return ((table.head ?? []) + table.rows.flatMap { $0 }).flatMap { $0 }.map(length).reduce(0, +)
        default:
            let inner = element.children ?? []
            return inner.map(length).reduce(0, +) + (separates(element) ? max(inner.count - 1, 0) : 0)
        }
    }

    static func separates(_ element: MdElement) -> Bool {
        switch element {
        case .ul, .ol, .quote, .details: true
        default: false
        }
    }

    /// Where each node's words start; at the top, the line between blocks comes only once some words are shown.
    static func starts(_ nodes: [MdNode], separated: Bool, top: Bool, at: Int = 0) -> [Int] {
        var cursor = at
        return nodes.enumerated().map { index, node in
            if separated, index > 0, !top || cursor > at { cursor += 1 }
            let start = cursor
            cursor += length(node)
            return start
        }
    }

    private static func words(_ node: MdNode) -> String {
        switch node {
        case .text(let text): return text
        case .element(let element):
            switch element {
            case .code(let code): return code.v
            case .pre(let pre): return pre.v
            case .br: return "\n"
            case .img, .hr, .fnref: return ""
            case .table(let table):
                return ((table.head ?? []) + table.rows.flatMap { $0 }).flatMap { $0 }.map(words).joined()
            default: return (element.children ?? []).map(words).joined(separator: separates(element) ? "\n" : "")
            }
        }
    }

    /// The words a part's tree shows, as the host finds in them.
    static func shown(_ blocks: [MdElement]) -> String {
        blocks.map { words(.element($0)) }.reduce("") { $0.isEmpty ? $1 : "\($0)\n\($1)" }
    }
}

/// What the find bar found in the rows held, by part, and the place the reader is on.
struct Finding: Equatable {
    var byPart: [String: [Range<Int>]] = [:]
    var current: (part: String, range: Range<Int>)?

    static func == (a: Finding, b: Finding) -> Bool {
        a.byPart == b.byPart && a.current?.part == b.current?.part && a.current?.range == b.current?.range
    }

    func marks(_ part: String) -> Marks? {
        guard let ranges = byPart[part], !ranges.isEmpty else { return nil }
        return Marks(ranges: ranges, current: current?.part == part ? current?.range.lowerBound : nil)
    }

    /// A sent message's words with the places found in its tree's words marked where they stand in the text as typed.
    func marked(_ parts: [ChatPart.Text], palette: Palette) -> AttributedString {
        var out = AttributedString()
        for (index, part) in parts.enumerated() {
            if index > 0 { out += AttributedString("\n\n") }
            out += marked(part, palette: palette)
        }
        return out
    }

    private func marked(_ part: ChatPart.Text, palette: Palette) -> AttributedString {
        let text = part.text
        var out = AttributedString(text)
        guard let marks = marks(part.id) else { return out }
        let typedUnits = Array(text.utf16)
        let shownUnits = Array((part.md.map { FindText.shown($0.blocks) } ?? text).utf16)
        // Markup only drops characters, so each one shown is the next match in the text as typed.
        var typed: [Int] = []
        var cursor = 0
        for unit in shownUnits {
            var at = cursor
            while at < typedUnits.count, typedUnits[at] != unit { at += 1 }
            if at < typedUnits.count {
                typed.append(at)
                cursor = at + 1
            } else {
                typed.append(cursor)
            }
        }
        var last = 0
        for range in marks.ranges {
            let from = range.lowerBound < typed.count ? typed[range.lowerBound] : typedUnits.count
            let to = range.upperBound - 1 < typed.count ? typed[range.upperBound - 1] + 1 : typedUnits.count
            guard from >= last, to > from, let found = Range(NSRange(location: from, length: to - from), in: out) else {
                continue
            }
            let current = marks.current == range.lowerBound
            out[found].backgroundColor = current ? palette.accent : palette.accent.opacity(0.34)
            out[found].foregroundColor = current ? palette.sunken : palette.ink
            last = to
        }
        return out
    }
}
