import Foundation

/// A chip the draft holds, over the text it wrote, as every app keeps it: `agent:<id>`, `file:<path>`,
/// `folder:<path>`, `earlier:<cli>/<session>`, `tracked:<kind>:<number>:<url>` or `image:<attachment>`. Places count UTF-16 units, as
/// the text field does.
struct TextToken: Hashable, Sendable {
    var start: Int
    var end: Int
    var id: String
    var label: String?
}

/// A word that asks for something to name, and where it starts.
struct Typed: Hashable {
    enum Trigger: String {
        case agents = "@"
        case issues = "#"
        case pulls = "!"
        case files = "+"
    }
    var trigger: Trigger
    var start: Int
    var needle: String
}

/// What a message's chips send besides its words.
struct Named: Sendable {
    var paths: [String] = []
    var context: [ChatContext] = []
}

enum Mentions {
    static let agent = "agent:"
    static let file = "file:"
    static let folder = "folder:"
    static let earlier = "earlier:"
    static let tracked = "tracked:"
    /// A picture the message carries, named where it goes in the words.
    static let image = "image:"

    static func trackedId(_ kind: TrackedKind, _ number: Int, _ url: String) -> String {
        "\(tracked)\(kind.rawValue):\(number):\(url)"
    }

    private static let typedPattern = try! NSRegularExpression(pattern: "(^|\\s)([@+#!])(\\S*)$")

    /// The word at the end of `draft` that asks for something to name, unless it is a chip already.
    static func typed(_ draft: String, _ tokens: [TextToken]) -> Typed? {
        let text = draft as NSString
        guard let found = typedPattern.firstMatch(in: draft, range: NSRange(location: 0, length: text.length)) else {
            return nil
        }
        let start = found.range(at: 2).location
        if tokens.contains(where: { $0.start == start }) { return nil }
        guard let trigger = Typed.Trigger(rawValue: text.substring(with: found.range(at: 2))) else { return nil }
        return Typed(trigger: trigger, start: start, needle: text.substring(with: found.range(at: 3)))
    }

    /// `after` as an edit of `before` leaves the chips: those before or after the change move with it and those it
    /// cuts into go. Taking text out of a chip takes the whole chip out, as the Mac's draft does.
    static func edit(_ before: String, _ after: String, _ tokens: [TextToken]) -> (text: String, tokens: [TextToken]) {
        let old = Array(before.utf16)
        let new = Array(after.utf16)
        var start = 0
        while start < old.count && start < new.count && old[start] == new[start] { start += 1 }
        var tail = 0
        while tail < old.count - start && tail < new.count - start
            && old[old.count - 1 - tail] == new[new.count - 1 - tail]
        {
            tail += 1
        }
        var removedEnd = old.count - tail
        var text = after
        var length = new.count
        let cut = tokens.filter { $0.end > start && $0.start < removedEnd }
        if !cut.isEmpty && new.count - tail == start {
            start = min(start, cut.map(\.start).min()!)
            removedEnd = max(removedEnd, cut.map(\.end).max()!)
            let kept = Array(old[0..<start]) + Array(old[removedEnd...])
            text = String(decoding: kept, as: UTF16.self)
            length = kept.count
        }
        let moved = length - old.count
        let tokens = tokens.compactMap { token -> TextToken? in
            if token.end <= start { return token }
            if token.start >= removedEnd {
                return TextToken(start: token.start + moved, end: token.end + moved, id: token.id, label: token.label)
            }
            return nil
        }
        return (text, tokens)
    }

    /// The chips a draft from the host holds, whose places it counts in UTF-8 bytes; one that no longer fits its
    /// text is left out.
    static func tokens(_ text: String, _ kept: [DraftToken]?) -> [TextToken] {
        guard let kept else { return [] }
        let units = utf16Places(text)
        return kept.compactMap { token in
            guard let start = units[token.start], let end = units[token.end], end > start else { return nil }
            return TextToken(start: start, end: end, id: token.id, label: token.label)
        }
    }

    static func kept(_ text: String, _ tokens: [TextToken]) -> [DraftToken] {
        let utf16 = Array(text.utf16)
        let bytes = { (unit: Int) in String(decoding: utf16[0..<min(unit, utf16.count)], as: UTF16.self).utf8.count }
        return tokens.map { DraftToken(start: bytes($0.start), end: bytes($0.end), id: $0.id, label: $0.label) }
    }

    /// By UTF-8 byte offset, the UTF-16 place of each character boundary in `text`.
    private static func utf16Places(_ text: String) -> [Int: Int] {
        var places: [Int: Int] = [0: 0]
        var bytes = 0
        var units = 0
        for scalar in text.unicodeScalars {
            bytes += String(scalar).utf8.count
            units += scalar.utf16.count
            places[bytes] = units
        }
        return places
    }

    /// What the chips in `text` send besides its words: the files they name, and the agents, chats and issues for
    /// the host to read.
    static func outgoing(_ text: String, _ tokens: [TextToken], _ tracked: [String: ChatContext]) -> Named {
        var named = Named()
        let add = { (item: ChatContext) in
            if !named.context.contains(where: { $0.uri == item.uri }) { named.context.append(item) }
        }
        let units = Array(text.utf16)
        for token in tokens {
            let written = String(
                decoding: units[min(token.start, units.count)..<min(token.end, units.count)], as: UTF16.self)
            let id = token.id
            let path =
                id.hasPrefix(file)
                ? String(id.dropFirst(file.count)) : id.hasPrefix(folder) ? String(id.dropFirst(folder.count)) : nil
            if let item = tracked[id] {
                add(item)
            } else if let path {
                if !named.paths.contains(path) { named.paths.append(path) }
            } else if id.hasPrefix(agent) {
                add(ChatContext(uri: WireURIs.agentURI + id.dropFirst(agent.count), title: written, text: ""))
            } else if id.hasPrefix(earlier) {
                add(ChatContext(uri: WireURIs.earlierURI + id.dropFirst(earlier.count), title: written, text: ""))
            }
        }
        return named
    }
}

/// A draft that keeps its pictures' tags only in its words, as a new chat's does: `[Image #n]` names attachment n.
enum ImageTags {
    private static let pattern = try! NSRegularExpression(pattern: "\\[Image #(\\d+)\\]")

    /// `text` with `[Image #n]` written at `caret` for each of `numbers`, spaced from the words around it.
    static func written(_ text: String, at caret: Int?, numbers: [Int]) -> String {
        guard !numbers.isEmpty else { return text }
        let length = (text as NSString).length
        let at = min(caret ?? length, length)
        let before = text.units(0, at)
        let after = text.units(at, length)
        let lead = before.last.map { $0.isWhitespace } ?? true ? "" : " "
        let trail = after.first?.isWhitespace ?? false ? "" : " "
        return before + lead + numbers.map { "[Image #\($0)]" }.joined(separator: " ") + trail + after
    }

    /// `text` without the tag of attachment `removed`, the later ones counted down so they still name theirs.
    static func removing(_ removed: Int, from text: String) -> String {
        let result = NSMutableString(string: text)
        for match in pattern.matches(in: text, range: NSRange(location: 0, length: result.length)).reversed() {
            let number = Int((text as NSString).substring(with: match.range(at: 1))) ?? 0
            if number == removed {
                result.replaceCharacters(in: match.range, with: "")
            } else if number > removed {
                result.replaceCharacters(in: match.range, with: "[Image #\(number - 1)]")
            }
        }
        return result as String
    }

    /// The tags in `text` that name one of the `count` attachments, as chips to draw.
    static func tokens(in text: String, count: Int) -> [TextToken] {
        pattern.matches(in: text, range: NSRange(location: 0, length: (text as NSString).length)).compactMap { match in
            let number = Int((text as NSString).substring(with: match.range(at: 1))) ?? 0
            guard number >= 1, number <= count else { return nil }
            return TextToken(
                start: match.range.location, end: match.range.location + match.range.length,
                id: "\(Mentions.image)\(number)", label: "Image \(number)")
        }
    }
}

extension String {
    /// The text between two UTF-16 places.
    func units(_ start: Int, _ end: Int) -> String {
        let text = self as NSString
        let from = min(start, text.length)
        return text.substring(with: NSRange(location: from, length: max(0, min(end, text.length) - from)))
    }
}
