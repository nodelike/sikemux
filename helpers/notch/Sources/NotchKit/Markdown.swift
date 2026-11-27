import AppKit
import Foundation
import SwiftUI

enum MarkdownBlock: Equatable {
    struct ListItem: Equatable {
        let indent: Int
        let content: [MarkdownInline]
    }

    struct OrderedItem: Equatable {
        let indent: Int
        let number: String
        let content: [MarkdownInline]
    }

    case paragraph([MarkdownInline])
    case heading(level: Int, [MarkdownInline])
    case bulletList([ListItem])
    case numberedList([OrderedItem])
    case quote([MarkdownInline])
    case codeBlock(String)
}

indirect enum MarkdownInline: Equatable {
    case text(String)
    case bold([MarkdownInline])
    case italic([MarkdownInline])
    case boldItalic([MarkdownInline])
    case strikethrough([MarkdownInline])
    case code(String)
    case link(label: [MarkdownInline], url: String)
    case lineBreak
}

struct MarkdownStyle {
    let size: CGFloat
    let weight: Font.Weight
    let ink: Color
    let strongInk: Color
    let codeSize: CGFloat
    let codeInk: Color
    let codeGround: Color
    let linkInk: Color
    let lineSpacing: CGFloat
    let blockSpacing: CGFloat

    static var headline: MarkdownStyle {
        MarkdownStyle(
            size: 15,
            weight: .semibold,
            ink: Theme.ink,
            strongInk: Theme.ink,
            codeSize: 13.5,
            codeInk: Theme.accent,
            codeGround: Theme.accentSoft,
            linkInk: Theme.accent,
            lineSpacing: 2,
            blockSpacing: 6
        )
    }

    static var body: MarkdownStyle {
        MarkdownStyle(
            size: 12.5,
            weight: .regular,
            ink: Theme.inkSoft,
            strongInk: Theme.ink,
            codeSize: 11.5,
            codeInk: Theme.accent,
            codeGround: Theme.accentSoft,
            linkInk: Theme.accent,
            lineSpacing: 2.5,
            blockSpacing: 7
        )
    }

    static var dim: MarkdownStyle {
        MarkdownStyle(
            size: 12.5,
            weight: .regular,
            ink: Theme.inkDim,
            strongInk: Theme.inkSoft,
            codeSize: 11.5,
            codeInk: Theme.accent,
            codeGround: Theme.accentSoft,
            linkInk: Theme.accent,
            lineSpacing: 2.5,
            blockSpacing: 6
        )
    }

    static var optionLabel: MarkdownStyle {
        MarkdownStyle(
            size: 13,
            weight: .regular,
            ink: Theme.ink,
            strongInk: Theme.ink,
            codeSize: 12,
            codeInk: Theme.accent,
            codeGround: Theme.accentSoft,
            linkInk: Theme.accent,
            lineSpacing: 2,
            blockSpacing: 4
        )
    }

    static var optionDetail: MarkdownStyle {
        MarkdownStyle(
            size: 11.5,
            weight: .regular,
            ink: Theme.inkDim,
            strongInk: Theme.inkSoft,
            codeSize: 10.5,
            codeInk: Theme.accent,
            codeGround: Theme.accentSoft,
            linkInk: Theme.accent,
            lineSpacing: 2,
            blockSpacing: 4
        )
    }
}

enum Markdown {
    private static let allowedSchemes: Set<String> = ["http", "https", "mailto", "sikemux"]

    /// Splits an ask question into its lead headline line and optional markdown body below it.
    static func questionParts(_ source: String) -> (headline: String?, body: String?) {
        let normalized = normalizeNewlines(source)
        let lines = normalized.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
        guard let firstIndex = lines.firstIndex(where: { !$0.trimmingCharacters(in: .whitespaces).isEmpty }) else {
            return (nil, nil)
        }
        let firstLine = lines[firstIndex]
        if startsStructuredBlock(firstLine) {
            let whole = lines[firstIndex...].joined(separator: "\n").trimmingCharacters(in: .whitespacesAndNewlines)
            return (nil, whole.isEmpty ? nil : whole)
        }
        let headline = stripHeadingPrefix(firstLine.trimmingCharacters(in: .whitespaces))
        let restLines = lines.dropFirst(firstIndex + 1).drop(while: { $0.trimmingCharacters(in: .whitespaces).isEmpty })
        let rest = restLines.joined(separator: "\n").trimmingCharacters(in: .whitespacesAndNewlines)
        return (headline.isEmpty ? nil : headline, rest.isEmpty ? nil : rest)
    }

    /// Strips markdown syntax down to a single clean line for compact collapsed rows.
    static func plainLine(_ source: String) -> String {
        let blocks = parseBlocks(source)
        for block in blocks {
            let text: String
            switch block {
            case .paragraph(let inlines), .heading(_, let inlines), .quote(let inlines):
                text = plainText(inlines)
            case .bulletList(let items):
                text = items.first.map { plainText($0.content) } ?? ""
            case .numberedList(let items):
                text = items.first.map { plainText($0.content) } ?? ""
            case .codeBlock(let code):
                text = code.split(separator: "\n").map { $0.trimmingCharacters(in: .whitespaces) }.first(where: { !$0.isEmpty }) ?? ""
            }
            let collapsed = collapseWhitespace(text)
            if !collapsed.isEmpty { return collapsed }
        }
        return collapseWhitespace(source)
    }

    static func plainText(_ inlines: [MarkdownInline]) -> String {
        var out = ""
        for node in inlines {
            switch node {
            case .text(let s), .code(let s):
                out += s
            case .bold(let children), .italic(let children), .boldItalic(let children),
                 .strikethrough(let children), .link(let children, _):
                out += plainText(children)
            case .lineBreak:
                out += " "
            }
        }
        return out
    }

    static func parseBlocks(_ source: String) -> [MarkdownBlock] {
        let normalized = normalizeNewlines(source)
        let lines = normalized.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
        var blocks: [MarkdownBlock] = []
        var index = 0

        while index < lines.count {
            let line = lines[index]
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if trimmed.isEmpty {
                index += 1
                continue
            }

            if trimmed.hasPrefix("```") {
                index += 1
                var codeLines: [String] = []
                while index < lines.count {
                    if lines[index].trimmingCharacters(in: .whitespaces).hasPrefix("```") {
                        index += 1
                        break
                    }
                    codeLines.append(lines[index])
                    index += 1
                }
                let code = codeLines.joined(separator: "\n")
                if !code.isEmpty { blocks.append(.codeBlock(code)) }
                continue
            }

            if let heading = parseHeading(trimmed) {
                blocks.append(.heading(level: heading.level, parseInlines(heading.text)))
                index += 1
                continue
            }

            if parseBullet(line) != nil {
                var items: [MarkdownBlock.ListItem] = []
                while index < lines.count {
                    guard let bullet = parseBullet(lines[index]) else { break }
                    var parts = [bullet.text]
                    index += 1
                    while index < lines.count {
                        let next = lines[index]
                        let nextTrimmed = next.trimmingCharacters(in: .whitespaces)
                        if nextTrimmed.isEmpty || startsAnyBlock(next) { break }
                        parts.append(nextTrimmed)
                        index += 1
                    }
                    items.append(MarkdownBlock.ListItem(indent: bullet.indent, content: parseInlines(joinSoftLines(parts))))
                }
                blocks.append(.bulletList(items))
                continue
            }

            if parseOrdered(line) != nil {
                var items: [MarkdownBlock.OrderedItem] = []
                while index < lines.count {
                    guard let ordered = parseOrdered(lines[index]) else { break }
                    var parts = [ordered.text]
                    index += 1
                    while index < lines.count {
                        let next = lines[index]
                        let nextTrimmed = next.trimmingCharacters(in: .whitespaces)
                        if nextTrimmed.isEmpty || startsAnyBlock(next) { break }
                        parts.append(nextTrimmed)
                        index += 1
                    }
                    items.append(MarkdownBlock.OrderedItem(indent: ordered.indent, number: ordered.number, content: parseInlines(joinSoftLines(parts))))
                }
                blocks.append(.numberedList(items))
                continue
            }

            if parseQuote(trimmed) != nil {
                var parts: [String] = []
                while index < lines.count {
                    let candidate = lines[index].trimmingCharacters(in: .whitespaces)
                    guard let quoted = parseQuote(candidate) else { break }
                    if !quoted.isEmpty { parts.append(quoted) }
                    index += 1
                }
                if !parts.isEmpty {
                    blocks.append(.quote(parseInlines(joinSoftLines(parts))))
                }
                continue
            }

            var paragraphLines: [String] = []
            while index < lines.count {
                let next = lines[index]
                let nextTrimmed = next.trimmingCharacters(in: .whitespaces)
                if nextTrimmed.isEmpty { break }
                if !paragraphLines.isEmpty && startsAnyBlock(next) { break }
                paragraphLines.append(next)
                index += 1
            }
            let joined = joinSoftLines(paragraphLines)
            if !joined.isEmpty {
                blocks.append(.paragraph(parseInlines(joined)))
            }
        }

        return blocks
    }

    static func parseInlines(_ source: String) -> [MarkdownInline] {
        let chars = Array(source)
        return parseInlineRange(chars, from: 0, to: chars.count)
    }

    static func attributed(_ inlines: [MarkdownInline], style: MarkdownStyle, allowLinks: Bool = true) -> AttributedString {
        var result = AttributedString()
        append(inlines, to: &result, style: style, bold: style.weight == .semibold || style.weight == .bold, italic: false, struck: false, linkURL: nil, allowLinks: allowLinks)
        return result
    }

    // MARK: - Block helpers

    private static func normalizeNewlines(_ s: String) -> String {
        s.replacingOccurrences(of: "\r\n", with: "\n").replacingOccurrences(of: "\r", with: "\n")
    }

    private static func startsStructuredBlock(_ line: String) -> Bool {
        let trimmed = line.trimmingCharacters(in: .whitespaces)
        return trimmed.hasPrefix("```")
            || parseBullet(line) != nil
            || parseOrdered(line) != nil
            || parseQuote(trimmed) != nil
    }

    private static func startsAnyBlock(_ line: String) -> Bool {
        let trimmed = line.trimmingCharacters(in: .whitespaces)
        return trimmed.hasPrefix("```")
            || parseHeading(trimmed) != nil
            || parseBullet(line) != nil
            || parseOrdered(line) != nil
            || parseQuote(trimmed) != nil
    }

    private static func stripHeadingPrefix(_ line: String) -> String {
        parseHeading(line)?.text ?? line
    }

    private static func parseHeading(_ trimmed: String) -> (level: Int, text: String)? {
        var count = 0
        var idx = trimmed.startIndex
        while idx < trimmed.endIndex && trimmed[idx] == "#" && count < 6 {
            count += 1
            idx = trimmed.index(after: idx)
        }
        guard count > 0 else { return nil }
        if idx == trimmed.endIndex { return (count, "") }
        guard trimmed[idx] == " " || trimmed[idx] == "\t" else { return nil }
        var rest = trimmed[idx...].trimmingCharacters(in: .whitespaces)
        while rest.hasSuffix("#") {
            let without = rest.dropLast()
            if without.isEmpty || without.last == " " || without.last == "\t" {
                rest = without.trimmingCharacters(in: .whitespaces)
            } else {
                break
            }
        }
        return (count, rest)
    }

    private static func leadingSpaces(_ line: String) -> (spaces: Int, rest: Substring) {
        var spaces = 0
        var idx = line.startIndex
        while idx < line.endIndex {
            if line[idx] == " " {
                spaces += 1
                idx = line.index(after: idx)
            } else if line[idx] == "\t" {
                spaces += 2
                idx = line.index(after: idx)
            } else {
                break
            }
        }
        return (spaces, line[idx...])
    }

    private static func parseBullet(_ line: String) -> (indent: Int, text: String)? {
        let (spaces, rest) = leadingSpaces(line)
        guard let first = rest.first, first == "-" || first == "*" || first == "+" else { return nil }
        let after = rest.index(after: rest.startIndex)
        guard after < rest.endIndex, rest[after] == " " || rest[after] == "\t" else { return nil }
        let content = rest[rest.index(after: after)...].trimmingCharacters(in: .whitespaces)
        guard !content.isEmpty else { return nil }
        return (min(spaces / 2, 3), content)
    }

    private static func parseOrdered(_ line: String) -> (indent: Int, number: String, text: String)? {
        let (spaces, rest) = leadingSpaces(line)
        var idx = rest.startIndex
        var digits = 0
        while idx < rest.endIndex && rest[idx].isASCII && rest[idx].isNumber && digits < 9 {
            digits += 1
            idx = rest.index(after: idx)
        }
        guard digits > 0, idx < rest.endIndex, rest[idx] == "." || rest[idx] == ")" else { return nil }
        let number = String(rest[rest.startIndex..<idx])
        let afterMarker = rest.index(after: idx)
        guard afterMarker < rest.endIndex, rest[afterMarker] == " " || rest[afterMarker] == "\t" else { return nil }
        let content = rest[rest.index(after: afterMarker)...].trimmingCharacters(in: .whitespaces)
        guard !content.isEmpty else { return nil }
        return (min(spaces / 2, 3), number, content)
    }

    private static func parseQuote(_ trimmed: String) -> String? {
        guard trimmed.hasPrefix(">") else { return nil }
        let after = trimmed.dropFirst()
        if after.isEmpty { return "" }
        if after.first == " " || after.first == "\t" {
            return after.dropFirst().trimmingCharacters(in: .whitespaces)
        }
        return after.trimmingCharacters(in: .whitespaces)
    }

    private static func joinSoftLines(_ lines: [String]) -> String {
        var out = ""
        for (i, raw) in lines.enumerated() {
            let hardBreak = raw.hasSuffix("  ") || raw.hasSuffix("\\")
            var piece = raw.trimmingCharacters(in: .whitespaces)
            if raw.hasSuffix("\\") && !piece.isEmpty { piece.removeLast() }
            out += piece
            if i + 1 < lines.count {
                out += hardBreak ? "\n" : " "
            }
        }
        return out
    }

    private static func collapseWhitespace(_ s: String) -> String {
        s.split(whereSeparator: \.isWhitespace).joined(separator: " ")
    }

    // MARK: - Inline parser

    private static func parseInlineRange(_ chars: [Character], from start: Int, to end: Int) -> [MarkdownInline] {
        var nodes: [MarkdownInline] = []
        var buffer = ""
        var i = start

        func flushBuffer() {
            if !buffer.isEmpty {
                nodes.append(.text(buffer))
                buffer = ""
            }
        }

        while i < end {
            let c = chars[i]

            if c == "\n" {
                flushBuffer()
                nodes.append(.lineBreak)
                i += 1
                continue
            }

            if c == "\\" && i + 1 < end && isEscapable(chars[i + 1]) {
                buffer.append(chars[i + 1])
                i += 2
                continue
            }

            if c == "`" {
                var ticks = 0
                while i + ticks < end && chars[i + ticks] == "`" { ticks += 1 }
                if let close = findClosingTicks(chars, count: ticks, from: i + ticks, to: end) {
                    flushBuffer()
                    var raw = String(chars[(i + ticks)..<close])
                    if raw.count >= 2 && raw.first == " " && raw.last == " " && raw.contains(where: { $0 != " " }) {
                        raw.removeFirst()
                        raw.removeLast()
                    }
                    nodes.append(.code(raw))
                    i = close + ticks
                    continue
                }
                buffer.append(contentsOf: repeatElement("`", count: ticks))
                i += ticks
                continue
            }

            if c == "[", let link = parseLink(chars, from: i, to: end) {
                flushBuffer()
                let label = parseInlineRange(chars, from: link.labelStart, to: link.labelEnd)
                nodes.append(.link(label: label, url: link.url))
                i = link.nextIndex
                continue
            }

            if c == "<", let autolink = parseAutolink(chars, from: i, to: end) {
                flushBuffer()
                nodes.append(.link(label: [.text(autolink.url)], url: autolink.url))
                i = autolink.nextIndex
                continue
            }

            if c == "~" && i + 1 < end && chars[i + 1] == "~" {
                if let close = findClosingDelimiter(chars, delimiter: "~", count: 2, from: i + 2, to: end, intrawordAllowed: true) {
                    flushBuffer()
                    nodes.append(.strikethrough(parseInlineRange(chars, from: i + 2, to: close)))
                    i = close + 2
                    continue
                }
            }

            if c == "*" || c == "_" {
                let intrawordAllowed = (c == "*")
                let prevIsWord = i > start && isWordChar(chars[i - 1])
                if !(!intrawordAllowed && prevIsWord) {
                    var run = 0
                    while i + run < end && chars[i + run] == c && run < 3 { run += 1 }
                    if run == 3, let close = findClosingDelimiter(chars, delimiter: c, count: 3, from: i + 3, to: end, intrawordAllowed: intrawordAllowed) {
                        flushBuffer()
                        nodes.append(.boldItalic(parseInlineRange(chars, from: i + 3, to: close)))
                        i = close + 3
                        continue
                    }
                    if run >= 2, let close = findClosingDelimiter(chars, delimiter: c, count: 2, from: i + 2, to: end, intrawordAllowed: intrawordAllowed) {
                        flushBuffer()
                        nodes.append(.bold(parseInlineRange(chars, from: i + 2, to: close)))
                        i = close + 2
                        continue
                    }
                    if run >= 1, let close = findClosingDelimiter(chars, delimiter: c, count: 1, from: i + 1, to: end, intrawordAllowed: intrawordAllowed) {
                        flushBuffer()
                        nodes.append(.italic(parseInlineRange(chars, from: i + 1, to: close)))
                        i = close + 1
                        continue
                    }
                }
            }

            buffer.append(c)
            i += 1
        }

        flushBuffer()
        return nodes
    }

    private static func isEscapable(_ c: Character) -> Bool {
        "\\`*_{}[]()#+-.!~|<>".contains(c)
    }

    private static func isWordChar(_ c: Character) -> Bool {
        c.isLetter || c.isNumber
    }

    private static func findClosingTicks(_ chars: [Character], count: Int, from start: Int, to end: Int) -> Int? {
        var i = start
        while i < end {
            if chars[i] == "`" {
                var run = 0
                while i + run < end && chars[i + run] == "`" { run += 1 }
                if run == count { return i }
                i += run
            } else {
                i += 1
            }
        }
        return nil
    }

    private static func findClosingDelimiter(
        _ chars: [Character],
        delimiter: Character,
        count: Int,
        from start: Int,
        to end: Int,
        intrawordAllowed: Bool
    ) -> Int? {
        guard start < end, !chars[start].isWhitespace else { return nil }
        var i = start
        while i + count <= end {
            let c = chars[i]
            if c == "\\" && i + 1 < end {
                i += 2
                continue
            }
            if c == "`" {
                var ticks = 0
                while i + ticks < end && chars[i + ticks] == "`" { ticks += 1 }
                if let close = findClosingTicks(chars, count: ticks, from: i + ticks, to: end) {
                    i = close + ticks
                    continue
                }
                i += ticks
                continue
            }
            if c == delimiter {
                var run = 0
                while i + run < end && chars[i + run] == delimiter { run += 1 }
                if run >= count && i > start && !chars[i - 1].isWhitespace {
                    let matchStart = i + run - count
                    let afterIndex = matchStart + count
                    let nextIsWord = afterIndex < end && isWordChar(chars[afterIndex])
                    if intrawordAllowed || !nextIsWord {
                        if matchStart > start {
                            return matchStart
                        }
                    }
                }
                i += run
                continue
            }
            i += 1
        }
        return nil
    }

    private static func parseLink(_ chars: [Character], from start: Int, to end: Int) -> (labelStart: Int, labelEnd: Int, url: String, nextIndex: Int)? {
        var i = start + 1
        var depth = 1
        var closeBracket: Int?
        while i < end {
            let c = chars[i]
            if c == "\\" && i + 1 < end {
                i += 2
                continue
            }
            if c == "`" {
                var ticks = 0
                while i + ticks < end && chars[i + ticks] == "`" { ticks += 1 }
                if let close = findClosingTicks(chars, count: ticks, from: i + ticks, to: end) {
                    i = close + ticks
                    continue
                }
                i += ticks
                continue
            }
            if c == "[" {
                depth += 1
            } else if c == "]" {
                depth -= 1
                if depth == 0 {
                    closeBracket = i
                    break
                }
            }
            i += 1
        }
        guard let labelEnd = closeBracket, labelEnd > start + 1 else { return nil }
        let openParen = labelEnd + 1
        guard openParen < end, chars[openParen] == "(" else { return nil }

        var j = openParen + 1
        while j < end && chars[j].isWhitespace { j += 1 }
        guard j < end else { return nil }

        let urlStart: Int
        let urlEnd: Int
        let closeParen: Int
        if chars[j] == "<" {
            urlStart = j + 1
            var k = urlStart
            while k < end && chars[k] != ">" && chars[k] != "\n" { k += 1 }
            guard k < end, chars[k] == ">" else { return nil }
            urlEnd = k
            k += 1
            while k < end && chars[k].isWhitespace { k += 1 }
            guard k < end, chars[k] == ")" else { return nil }
            closeParen = k
        } else {
            urlStart = j
            var parens = 0
            var k = j
            var foundClose: Int?
            while k < end {
                let ch = chars[k]
                if ch == "\\" && k + 1 < end {
                    k += 2
                    continue
                }
                if ch == "(" {
                    parens += 1
                } else if ch == ")" {
                    if parens == 0 {
                        foundClose = k
                        break
                    }
                    parens -= 1
                } else if ch.isWhitespace {
                    break
                }
                k += 1
            }
            guard let found = foundClose else { return nil }
            urlEnd = found
            closeParen = found
        }

        let url = String(chars[urlStart..<urlEnd]).trimmingCharacters(in: .whitespaces)
        guard !url.isEmpty else { return nil }
        return (start + 1, labelEnd, url, closeParen + 1)
    }

    private static func parseAutolink(_ chars: [Character], from start: Int, to end: Int) -> (url: String, nextIndex: Int)? {
        var i = start + 1
        while i < end && chars[i] != ">" && !chars[i].isWhitespace { i += 1 }
        guard i < end, chars[i] == ">", i > start + 1 else { return nil }
        let candidate = String(chars[(start + 1)..<i])
        guard validURL(candidate) != nil else { return nil }
        return (candidate, i + 1)
    }

    private static func validURL(_ raw: String) -> URL? {
        guard let url = URL(string: raw),
              let scheme = url.scheme?.lowercased(),
              allowedSchemes.contains(scheme)
        else { return nil }
        return url
    }

    // MARK: - AttributedString rendering

    private static func append(
        _ inlines: [MarkdownInline],
        to out: inout AttributedString,
        style: MarkdownStyle,
        bold: Bool,
        italic: Bool,
        struck: Bool,
        linkURL: URL?,
        allowLinks: Bool
    ) {
        for node in inlines {
            switch node {
            case .text(let s):
                var part = AttributedString(s)
                let weight: Font.Weight = bold ? .semibold : style.weight
                part.font = italic ? Theme.uiItalic(style.size, weight) : Theme.ui(style.size, weight)
                if let linkURL, allowLinks {
                    part.link = linkURL
                    part.foregroundColor = style.linkInk
                    part.underlineStyle = Text.LineStyle(pattern: .solid, color: style.linkInk.opacity(0.45))
                } else {
                    part.foregroundColor = bold ? style.strongInk : style.ink
                }
                if struck {
                    part.strikethroughStyle = .single
                }
                out.append(part)

            case .code(let code):
                var part = AttributedString(code)
                part.font = Theme.mono(style.codeSize, bold ? .medium : .regular)
                if let linkURL, allowLinks {
                    part.link = linkURL
                    part.foregroundColor = style.linkInk
                    part.underlineStyle = Text.LineStyle(pattern: .solid, color: style.linkInk.opacity(0.45))
                } else {
                    part.foregroundColor = style.codeInk
                }
                part.backgroundColor = style.codeGround
                if struck {
                    part.strikethroughStyle = .single
                }
                out.append(part)

            case .bold(let children):
                append(children, to: &out, style: style, bold: true, italic: italic, struck: struck, linkURL: linkURL, allowLinks: allowLinks)

            case .italic(let children):
                append(children, to: &out, style: style, bold: bold, italic: true, struck: struck, linkURL: linkURL, allowLinks: allowLinks)

            case .boldItalic(let children):
                append(children, to: &out, style: style, bold: true, italic: true, struck: struck, linkURL: linkURL, allowLinks: allowLinks)

            case .strikethrough(let children):
                append(children, to: &out, style: style, bold: bold, italic: italic, struck: true, linkURL: linkURL, allowLinks: allowLinks)

            case .link(let label, let rawURL):
                let resolved = allowLinks ? validURL(rawURL) : nil
                append(label, to: &out, style: style, bold: bold, italic: italic, struck: struck, linkURL: resolved ?? linkURL, allowLinks: allowLinks)

            case .lineBreak:
                out.append(AttributedString("\n"))
            }
        }
    }
}

struct InlineMarkdownText: View {
    let text: String
    let style: MarkdownStyle
    let prefix: (text: String, color: Color)?
    let allowLinks: Bool
    let onOpenURL: ((URL) -> Void)?

    init(
        _ text: String,
        style: MarkdownStyle,
        prefix: (text: String, color: Color)? = nil,
        allowLinks: Bool = true,
        onOpenURL: ((URL) -> Void)? = nil
    ) {
        self.text = text
        self.style = style
        self.prefix = prefix
        self.allowLinks = allowLinks
        self.onOpenURL = onOpenURL
    }

    private var rendered: AttributedString {
        let content = Markdown.attributed(Markdown.parseInlines(text), style: style, allowLinks: allowLinks)
        guard let prefix else { return content }
        var head = AttributedString(prefix.text)
        head.font = Theme.ui(style.size, style.weight)
        head.foregroundColor = prefix.color
        return head + content
    }

    var body: some View {
        Text(rendered)
            .lineSpacing(style.lineSpacing)
            .fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: .infinity, alignment: .leading)
            .environment(\.openURL, OpenURLAction { url in
                if let onOpenURL {
                    onOpenURL(url)
                } else {
                    NSWorkspace.shared.open(url)
                }
                return .handled
            })
    }
}

struct MarkdownView: View {
    let text: String
    let style: MarkdownStyle
    var onOpenURL: ((URL) -> Void)? = nil

    var body: some View {
        let blocks = Markdown.parseBlocks(text)
        VStack(alignment: .leading, spacing: style.blockSpacing) {
            ForEach(Array(blocks.enumerated()), id: \.offset) { _, block in
                render(block)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .environment(\.openURL, OpenURLAction { url in
            if let onOpenURL {
                onOpenURL(url)
            } else {
                NSWorkspace.shared.open(url)
            }
            return .handled
        })
    }

    @ViewBuilder
    private func render(_ block: MarkdownBlock) -> some View {
        switch block {
        case .paragraph(let inlines):
            Text(Markdown.attributed(inlines, style: style))
                .lineSpacing(style.lineSpacing)
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: .infinity, alignment: .leading)

        case .heading(let level, let inlines):
            let headingStyle = MarkdownStyle(
                size: level <= 2 ? style.size + 1.5 : style.size + 0.5,
                weight: .semibold,
                ink: Theme.ink,
                strongInk: Theme.ink,
                codeSize: level <= 2 ? style.codeSize + 1 : style.codeSize,
                codeInk: style.codeInk,
                codeGround: style.codeGround,
                linkInk: style.linkInk,
                lineSpacing: 2,
                blockSpacing: style.blockSpacing
            )
            Text(Markdown.attributed(inlines, style: headingStyle))
                .lineSpacing(2)
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: .infinity, alignment: .leading)

        case .bulletList(let items):
            VStack(alignment: .leading, spacing: 5) {
                ForEach(Array(items.enumerated()), id: \.offset) { _, item in
                    HStack(alignment: .firstTextBaseline, spacing: 6) {
                        Text("•")
                            .font(Theme.ui(style.size))
                            .foregroundStyle(Theme.inkDim)
                            .frame(width: 8, alignment: .center)
                        Text(Markdown.attributed(item.content, style: style))
                            .lineSpacing(style.lineSpacing)
                            .fixedSize(horizontal: false, vertical: true)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    .padding(.leading, CGFloat(item.indent) * 12)
                }
            }

        case .numberedList(let items):
            VStack(alignment: .leading, spacing: 5) {
                ForEach(Array(items.enumerated()), id: \.offset) { _, item in
                    HStack(alignment: .firstTextBaseline, spacing: 6) {
                        Text("\(item.number).")
                            .font(Theme.ui(style.size))
                            .monospacedDigit()
                            .foregroundStyle(Theme.inkDim)
                            .fixedSize()
                        Text(Markdown.attributed(item.content, style: style))
                            .lineSpacing(style.lineSpacing)
                            .fixedSize(horizontal: false, vertical: true)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    .padding(.leading, CGFloat(item.indent) * 12)
                }
            }

        case .quote(let inlines):
            let quoteStyle = MarkdownStyle(
                size: style.size,
                weight: .regular,
                ink: Theme.inkDim,
                strongInk: Theme.inkSoft,
                codeSize: style.codeSize,
                codeInk: style.codeInk,
                codeGround: style.codeGround,
                linkInk: style.linkInk,
                lineSpacing: style.lineSpacing,
                blockSpacing: style.blockSpacing
            )
            HStack(alignment: .top, spacing: 8) {
                RoundedRectangle(cornerRadius: 1)
                    .fill(Theme.hairline)
                    .frame(width: 2)
                Text(Markdown.attributed(inlines, style: quoteStyle))
                    .lineSpacing(style.lineSpacing)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .fixedSize(horizontal: false, vertical: true)

        case .codeBlock(let code):
            Text(code)
                .font(Theme.mono(max(11, style.codeSize)))
                .foregroundStyle(Theme.inkSoft)
                .lineSpacing(2)
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 10)
                .padding(.vertical, 7)
                .background(RoundedRectangle(cornerRadius: 7).fill(Theme.codeBlockGround))
                .overlay(RoundedRectangle(cornerRadius: 7).strokeBorder(Theme.hairline, lineWidth: 1))
        }
    }
}
