import SwiftUI

/// What the find bar found in the rows held, by part, and the one it is on.
struct Finding: Equatable {
    var byPart: [String: [Range<Int>]] = [:]
    var current: (part: String, range: Range<Int>)?

    static func == (a: Finding, b: Finding) -> Bool {
        a.byPart == b.byPart && a.current?.part == b.current?.part && a.current?.range == b.current?.range
    }

    func marked(_ parts: [ChatPart.Text], palette: Palette) -> AttributedString {
        AttributedString(parts.map(\.text).joined(separator: "\n\n"))
    }

    func marking(_ blocks: [MdElement], part: String) -> [MdElement] {
        blocks
    }
}
