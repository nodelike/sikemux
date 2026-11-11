import SwiftUI
import UIKit

/// How a run of markdown is set: its size, line height, colour and face.
struct ProseStyle: Equatable {
    var size: CGFloat = 14.5
    var lineHeight: CGFloat = 23
    var color: Color
    var italic = false

    var font: Font { italic ? Theme.italic(size) : Theme.ui(size) }
    var spacing: CGFloat { max(0, lineHeight - size * 1.22) }

    static func prose(_ palette: Palette) -> ProseStyle { ProseStyle(color: palette.ink) }
    static func thought(_ palette: Palette) -> ProseStyle {
        ProseStyle(size: 12.5, lineHeight: 19.5, color: palette.inkFaint, italic: true)
    }
}

/// Links the transcript opens itself rather than handing to the system: files on the host.
enum TranscriptLink {
    static let fileScheme = "sikemux-file"

    static func file(_ raw: String) -> URL? {
        var parts = URLComponents()
        parts.scheme = fileScheme
        parts.host = "open"
        parts.queryItems = [URLQueryItem(name: "path", value: raw)]
        return parts.url
    }

    static func filePath(_ url: URL) -> String? {
        guard url.scheme == fileScheme else { return nil }
        return URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems?.first { $0.name == "path" }?.value
    }

    /// A link the phone opens: the web links the Mac would open, and mail.
    static func openable(_ raw: String) -> URL? {
        guard let url = URL(string: raw), let scheme = url.scheme?.lowercased() else { return nil }
        if scheme == "mailto" { return url }
        guard scheme == "http" || scheme == "https", url.user == nil, url.password == nil, url.host != nil else {
            return nil
        }
        return url
    }
}

/// Text from a host that sends no tree, as the paragraphs its blank lines make.
func plainBlocks(_ text: String) -> [MdElement] {
    text.replacingOccurrences(of: "\r\n", with: "\n")
        .components(separatedBy: "\n\n")
        .filter { !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
        .map { .p(.init(c: [.text($0)])) }
}

/// Words the core's tree spells out, inline: `code` as the Mac's purple chip, **bold** as semibold, links in the
/// accent, files as links the transcript opens.
struct Inline {
    let palette: Palette
    let style: ProseStyle
    var paths = true
    /// The places the find bar found, and where in the part's words this run starts.
    var marks: Marks?
    var cursor: FindCursor?

    func string(_ nodes: [MdNode]) -> AttributedString {
        var out = AttributedString()
        for node in nodes { out += string(node) }
        return out
    }

    /// Words as written; `counts` says whether the find bar counts them among the part's words.
    private func plain(_ text: String, counts: Bool = true) -> AttributedString {
        var run = AttributedString(text)
        guard counts, let marks, let cursor else { return run }
        let start = cursor.at
        let length = text.utf16.count
        cursor.at += length
        for range in marks.ranges where range.lowerBound < start + length && range.upperBound > start {
            let from = max(range.lowerBound - start, 0)
            let to = min(range.upperBound - start, length)
            guard to > from, let found = Range(NSRange(location: from, length: to - from), in: run) else { continue }
            let current = marks.current == range.lowerBound
            run[found].backgroundColor = current ? palette.accent : palette.accent.opacity(0.34)
            run[found].foregroundColor = current ? palette.sunken : palette.ink
        }
        return run
    }

    private func inner(_ paths: Bool) -> Inline {
        Inline(palette: palette, style: style, paths: paths, marks: marks, cursor: cursor)
    }

    private func string(_ node: MdNode) -> AttributedString {
        switch node {
        case .text(let text): return plain(text)
        case .element(let element): return string(element)
        }
    }

    func string(_ element: MdElement) -> AttributedString {
        switch element {
        case .code(let code):
            return chip(code.v)
        case .strong(let strong):
            var run = string(strong.c)
            run.font = Theme.ui(style.size, .semibold)
            run.foregroundColor = palette.ink
            return run
        case .em(let em):
            var run = string(em.c)
            run.font = Theme.italic(style.size)
            return run
        case .del(let del):
            var run = string(del.c)
            run.strikethroughStyle = .single
            run.foregroundColor = palette.tertiary
            return run
        case .br:
            return plain("\n")
        case .fnref(let ref):
            var run = plain("\(ref.n)", counts: false)
            run.font = Theme.ui(10)
            run.baselineOffset = 4
            run.foregroundColor = palette.accent
            return run
        case .path(let path):
            let label = path.code == true ? chip(visibleText(path.c)) : inner(false).string(path.c)
            guard paths, HostFiles.namesFile(path.href), let url = TranscriptLink.file(path.href) else { return label }
            var run = FileGlyph.mark(size: 12) + AttributedString(" ") + label
            run.link = url
            if path.code != true { run.foregroundColor = palette.accent }
            return run
        case .img(let image):
            if let url = TranscriptLink.openable(image.src) {
                var run = plain(image.alt.isEmpty ? image.src : image.alt, counts: false)
                run.link = url
                run.foregroundColor = palette.accent
                return run
            }
            if paths, let url = TranscriptLink.file(image.src) {
                var run = plain(image.alt.isEmpty ? image.src : image.alt, counts: false)
                run.link = url
                run.foregroundColor = palette.accent
                return run
            }
            return plain(image.alt, counts: false)
        case .a(let link):
            var label = inner(false).string(link.c)
            if let url = TranscriptLink.openable(link.href) {
                label.link = url
                label.foregroundColor = palette.accent
            } else if link.file == true, paths, let url = TranscriptLink.file(link.href) {
                label = FileGlyph.mark(size: 12) + AttributedString(" ") + label
                label.link = url
                label.foregroundColor = palette.accent
            }
            return label
        default:
            return element.children.map(string) ?? AttributedString()
        }
    }

    private func chip(_ text: String) -> AttributedString {
        var run = AttributedString(text)
        run.font = Theme.mono(12.5)
        run.foregroundColor = palette.accent
        run.backgroundColor = palette.accentSoft
        guard marks != nil else { return run }
        var marked = plain(text)
        marked.font = Theme.mono(12.5)
        let plainRanges = marked.runs.filter { $0.backgroundColor == nil }.map(\.range)
        for range in plainRanges {
            marked[range].foregroundColor = palette.accent
            marked[range].backgroundColor = palette.accentSoft
        }
        return marked
    }
}

func visibleText(_ nodes: [MdNode]) -> String {
    nodes.map { node in
        switch node {
        case .text(let text): text
        case .element(let element): visibleText(element)
        }
    }.joined()
}

func visibleText(_ element: MdElement) -> String {
    switch element {
    case .pre(let pre): pre.v
    case .code(let code): code.v
    case .br: "\n"
    case .table(let table): ((table.head ?? []) + table.rows.flatMap { $0 }).map(visibleText).joined()
    default: element.children.map(visibleText) ?? ""
    }
}

/// An answer's blocks, each drawn again only when the core changed it, so a streaming message redraws just its tail.
struct MarkdownView: View {
    @Environment(\.palette) private var palette
    let blocks: [MdElement]
    let style: ProseStyle
    /// The places the find bar found in this part, while it is open.
    var marks: Marks?
    /// The end still coming in, drawn fainter.
    var fading: Fading = []

    var body: some View {
        let starts = marks.map { _ in FindText.starts(blocks.map(MdNode.element), separated: true, top: true) }
        VStack(alignment: .leading, spacing: 10) {
            ForEach(Array(blocks.enumerated()), id: \.offset) { index, block in
                BlockView(
                    block: block, style: style, palette: palette, marks: marks?.from(starts![index]),
                    fading: index == blocks.count - 1 ? fading : []
                ).equatable()
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

private let headingSizes: [CGFloat] = [20, 18, 16.5]

struct BlockView: View, Equatable {
    let block: MdElement
    let style: ProseStyle
    let palette: Palette
    var marks: Marks?
    var fading: Fading = []

    nonisolated static func == (a: BlockView, b: BlockView) -> Bool {
        a.block == b.block && a.style == b.style && a.palette == b.palette && a.marks == b.marks
            && a.fading.map(\.text) == b.fading.map(\.text) && a.fading.map(\.opacity) == b.fading.map(\.opacity)
    }

    private var inline: Inline {
        Inline(palette: palette, style: style, marks: marks, cursor: marks.map { FindCursor($0.base) })
    }

    var body: some View {
        switch block {
        case .pre(let pre):
            CodeBlock(lang: pre.lang ?? "", text: pre.v)
        case .h(let heading):
            let size = heading.l <= headingSizes.count ? headingSizes[heading.l - 1] : style.size
            let set = ProseStyle(size: size, lineHeight: size * 1.35, color: palette.ink)
            let text = Inline(palette: palette, style: set, marks: marks, cursor: marks.map { FindCursor($0.base) })
                .string(heading.c)
                .settingAttributes(AttributeContainer().font(Theme.ui(size, .semibold)))
            Prose(text: text.fading(fading, color: set.color), style: set).accessibilityAddTraits(.isHeader)
        case .ul(let list):
            ListBlock(items: list.c, first: nil, style: style, palette: palette, marks: marks)
        case .ol(let list):
            ListBlock(items: list.c, first: list.start ?? 1, style: style, palette: palette, marks: marks)
        case .quote(let quote):
            let quoted = ProseStyle(
                size: style.size, lineHeight: style.lineHeight, color: palette.secondary, italic: style.italic)
            Contents(nodes: quote.c, style: quoted, palette: palette, marks: marks, separated: true)
                .padding(.leading, 12)
                .overlay(alignment: .leading) { Rectangle().fill(palette.borderStrong).frame(width: 2) }
        case .hr:
            Rectangle().fill(palette.borderStrong).frame(height: 1 / UIScreen.main.scale).padding(.vertical, 4)
        case .table(let table):
            TableBlock(table: table, style: style)
        case .fndef(let note):
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text("\(note.n)").font(Theme.ui(10)).foregroundStyle(palette.accent).frame(
                    minWidth: 14, alignment: .leading)
                Contents(
                    nodes: note.c,
                    style: ProseStyle(size: 12.5, lineHeight: style.lineHeight, color: palette.secondary),
                    palette: palette, marks: marks)
            }
            .padding(.leading, 2)
        case .details(let details):
            DetailsBlock(details: details, style: style, marks: marks)
        case .p(let paragraph):
            Prose(text: inline.string(paragraph.c).fading(fading, color: style.color), style: style)
        default:
            Prose(text: inline.string(block), style: style)
        }
    }
}

/// Words set in the transcript's prose, selectable as the Mac's are.
struct Prose: View {
    @Environment(\.palette) private var palette
    let text: AttributedString
    let style: ProseStyle

    var body: some View {
        FileGlyph.text(text, ink: palette.inkDim)
            .font(style.font)
            .foregroundStyle(style.color)
            .lineSpacing(style.spacing)
            .frame(maxWidth: .infinity, alignment: .leading)
            .fixedSize(horizontal: false, vertical: true)
    }
}

private func isBlock(_ node: MdNode) -> Bool {
    guard case .element(let element) = node else { return false }
    switch element {
    case .p, .h, .quote, .ul, .ol, .pre, .hr, .table, .fndef, .details: return true
    default: return false
    }
}

/// An item's or a footnote's contents: runs of words as one line, blocks under them.
struct Contents: View {
    let nodes: [MdNode]
    let style: ProseStyle
    let palette: Palette
    var marks: Marks?
    /// The find bar counts a line between each of these nodes, as between a quote's blocks.
    var separated = false

    private enum Group {
        case words([MdNode], Int)
        case block(MdElement, Int)
    }

    private var groups: [Group] {
        let starts = marks.map { FindText.starts(nodes, separated: separated, top: false, at: $0.base) } ?? []
        var groups: [Group] = []
        for (index, node) in nodes.enumerated() {
            let start = starts.isEmpty ? 0 : starts[index]
            if isBlock(node), case .element(let element) = node {
                groups.append(.block(element, start))
            } else if case .words(var run, let from) = groups.last {
                run.append(node)
                groups[groups.count - 1] = .words(run, from)
            } else {
                groups.append(.words([node], start))
            }
        }
        return groups
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            ForEach(Array(groups.enumerated()), id: \.offset) { _, group in
                switch group {
                case .words(let run, let start):
                    Prose(
                        text: Inline(
                            palette: palette, style: style, marks: marks?.from(start),
                            cursor: marks.map { _ in FindCursor(start) }
                        ).string(run), style: style)
                case .block(let block, let start):
                    BlockView(block: block, style: style, palette: palette, marks: marks?.from(start))
                }
            }
        }
    }
}

private struct ListBlock: View {
    let items: [MdNode]
    /// The first item's number, for a numbered list.
    let first: Int?
    let style: ProseStyle
    let palette: Palette
    var marks: Marks?

    var body: some View {
        let starts = marks.map { FindText.starts(items, separated: true, top: false, at: $0.base) } ?? []
        VStack(alignment: .leading, spacing: 10) {
            ForEach(Array(items.enumerated()), id: \.offset) { index, node in
                if case .element(.li(let item)) = node {
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        if let checked = item.checked {
                            ZStack {
                                RoundedRectangle(cornerRadius: 4)
                                    .fill(checked ? palette.selected : .clear)
                                    .strokeBorder(checked ? palette.borderSelected : palette.borderStrong, lineWidth: 1)
                                if checked { IconView(icon: Icons.check, size: 11).foregroundStyle(palette.accent) }
                            }
                            .frame(width: 15, height: 15)
                            .alignmentGuide(.firstTextBaseline) { $0.height - 3 }
                            .accessibilityLabel(checked ? "done" : "not done")
                        } else {
                            Text(first.map { "\($0 + index)." } ?? "•")
                                .font(style.font)
                                .foregroundStyle(palette.tertiary)
                                .frame(minWidth: 14, alignment: .leading)
                        }
                        Contents(
                            nodes: item.c,
                            style: item.checked == true
                                ? ProseStyle(
                                    size: style.size, lineHeight: style.lineHeight, color: palette.tertiary,
                                    italic: style.italic) : style, palette: palette,
                            marks: starts.isEmpty ? nil : marks?.from(starts[index]))
                    }
                    .padding(.leading, 2)
                }
            }
        }
    }
}

private struct DetailsBlock: View {
    @Environment(\.palette) private var palette
    let details: MdElement.Details
    let style: ProseStyle
    var marks: Marks?
    @State private var open: Bool?

    var body: some View {
        let shown = open ?? (details.open == true)
        VStack(alignment: .leading, spacing: 8) {
            Button {
                open = !shown
            } label: {
                HStack(spacing: 6) {
                    IconView(icon: Icons.chevron, size: 10)
                        .foregroundStyle(palette.inkFaint)
                        .rotationEffect(.degrees(shown ? 90 : 0))
                    Text(details.summary.isEmpty ? "Details" : details.summary)
                        .font(Theme.ui(style.size, .semibold))
                        .foregroundStyle(palette.ink)
                }
            }
            .buttonStyle(.plain)
            .accessibilityValue(shown ? "expanded" : "collapsed")
            if shown || marks != nil {
                Contents(nodes: details.c, style: style, palette: palette, marks: marks, separated: true)
            }
        }
    }
}

/// A fence as the Mac draws it: a title bar with the file it quotes and its copy button, then the code, or a patch's
/// rows tinted by sign with the changed words marked.
struct CodeBlock: View, Equatable {
    @Environment(\.palette) private var palette
    let lang: String
    let text: String

    nonisolated static func == (a: CodeBlock, b: CodeBlock) -> Bool { a.lang == b.lang && a.text == b.text }

    var body: some View {
        let name = lang.removingPercentEncoding ?? lang
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 6) {
                if !name.isEmpty { IconView(icon: Icons.file, size: 11).foregroundStyle(palette.inkFaint) }
                Text(name).font(Theme.mono(10.5)).foregroundStyle(palette.inkFaint).lineLimit(1)
                Spacer(minLength: 0)
                CopyButton(value: text.hasSuffix("\n") ? String(text.dropLast()) : text, label: "code", size: 12)
            }
            .padding(.leading, 10)
            .padding(.trailing, 1)
            .frame(height: 26)
            .overlay(alignment: .bottom) { Rectangle().fill(palette.border).frame(height: 1) }
            if let patch = DiffText.fenced(text, lang.isEmpty ? nil : lang) {
                DiffRows(lines: patch, numbered: false)
            } else {
                ScrollView(.horizontal, showsIndicators: false) {
                    Text(text)
                        .font(Theme.mono(11.5))
                        .lineSpacing(17 - 11.5 * 1.32)
                        .foregroundStyle(palette.ink)
                        .fixedSize()
                        .padding(.horizontal, 12)
                        .padding(.vertical, 9)
                }
            }
        }
        .background(palette.sunken)
        .clipShape(RoundedRectangle(cornerRadius: 8))
        .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(palette.border, lineWidth: 1))
    }
}

/// A table sized from its text up front, scrolling sideways in its own box when it is wider than the screen.
private struct TableBlock: View {
    @Environment(\.palette) private var palette
    let table: MdElement.Table
    let style: ProseStyle

    private static let cellChar: CGFloat = 7.4
    /// A heading cell is set in spaced capitals, wider than the rows' text.
    private static let headChar: CGFloat = 9
    private static let cellPad: CGFloat = 20
    private static let copyRoom: CGFloat = 28

    private var widths: [CGFloat] {
        table.align.indices.map { column in
            var width = CGFloat(visibleText(table.head?[safe: column] ?? []).count) * Self.headChar
            for row in table.rows {
                width = max(width, CGFloat(visibleText(row[safe: column] ?? []).count) * Self.cellChar)
            }
            let room = column == table.align.count - 1 ? Self.copyRoom : 0
            return min(240, max(56, width.rounded(.up) + Self.cellPad)) + room
        }
    }

    var body: some View {
        let widths = widths
        ScrollView(.horizontal, showsIndicators: false) {
            VStack(alignment: .leading, spacing: 0) {
                if let head = table.head {
                    row(head, widths: widths, head: true)
                        .background(palette.active)
                        .overlay(alignment: .bottom) { Rectangle().fill(palette.borderStrong).frame(height: 1) }
                }
                ForEach(Array(table.rows.enumerated()), id: \.offset) { index, cells in
                    row(cells, widths: widths, head: false)
                        .overlay(alignment: .bottom) {
                            if index < table.rows.count - 1 { Rectangle().fill(palette.border).frame(height: 1) }
                        }
                }
            }
        }
        .overlay(alignment: .topTrailing) {
            CopyButton(value: tableSource(table), label: "table", size: 12).background(palette.raised).padding(3)
        }
        .clipShape(RoundedRectangle(cornerRadius: 8))
        .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(palette.border, lineWidth: 1))
    }

    private func row(_ cells: [[MdNode]], widths: [CGFloat], head: Bool) -> some View {
        HStack(alignment: .top, spacing: 0) {
            ForEach(Array(widths.enumerated()), id: \.offset) { column, width in
                let nodes = cells[safe: column] ?? []
                let set =
                    head
                    ? ProseStyle(size: 12, lineHeight: 19, color: palette.secondary)
                    : ProseStyle(size: 13.5, lineHeight: 19, color: style.color)
                let words =
                    head
                    ? AttributedString(visibleText(nodes).uppercased())
                    : Inline(palette: palette, style: set).string(nodes)
                Text(words)
                    .font(head ? Theme.ui(12, .semibold) : set.font)
                    .tracking(head ? 0.6 : 0)
                    .foregroundStyle(set.color)
                    .lineSpacing(set.spacing)
                    .multilineTextAlignment(alignment(column))
                    .frame(width: width - 20, alignment: frameAlignment(column))
                    .padding(.horizontal, 10)
                    .padding(.vertical, 6)
                    .padding(.trailing, head && column == widths.count - 1 ? Self.copyRoom : 0)
                    .overlay(alignment: .trailing) {
                        if column < widths.count - 1 { Rectangle().fill(palette.border).frame(width: 1) }
                    }
            }
        }
    }

    private func alignment(_ column: Int) -> TextAlignment {
        switch table.align[safe: column] ?? nil {
        case .center: .center
        case .right: .trailing
        default: .leading
        }
    }

    private func frameAlignment(_ column: Int) -> Alignment {
        switch table.align[safe: column] ?? nil {
        case .center: .center
        case .right: .trailing
        default: .leading
        }
    }
}

/// The table written back as markdown, so it pastes into a doc or another agent as a table.
func tableSource(_ table: MdElement.Table) -> String {
    func inline(_ node: MdNode) -> String {
        switch node {
        case .text(let text): return text.replacingOccurrences(of: "|", with: "\\|")
        case .element(let element):
            switch element {
            case .code(let code): return "`\(code.v.replacingOccurrences(of: "|", with: "\\|"))`"
            case .strong(let e): return "**\(inlines(e.c))**"
            case .em(let e): return "*\(inlines(e.c))*"
            case .del(let e): return "~~\(inlines(e.c))~~"
            case .a(let e): return "[\(inlines(e.c))](\(e.href))"
            case .img(let e): return "![\(e.alt)](\(e.src))"
            case .br: return "<br>"
            case .fnref(let e): return "[^\(e.n)]"
            default: return element.children.map(inlines) ?? ""
            }
        }
    }
    func inlines(_ nodes: [MdNode]) -> String { nodes.map(inline).joined() }
    func row(_ cells: [[MdNode]]) -> String { "| " + cells.map(inlines).joined(separator: " | ") + " |" }
    let rule = table.align.map { align in
        switch align {
        case .center: ":---:"
        case .right: "---:"
        case .left: ":---"
        default: "---"
        }
    }
    let head = table.head ?? table.align.map { _ in [] }
    return ([row(head), "| " + rule.joined(separator: " | ") + " |"] + table.rows.map(row)).joined(separator: "\n")
}

extension Array {
    subscript(safe index: Int) -> Element? { indices.contains(index) ? self[index] : nil }
}

/// The file glyph a file the transcript names wears before its words, as the Mac's file refs do.
enum FileGlyph: AttributedStringKey {
    typealias Value = CGFloat
    static let name = "sikemux.fileGlyph"

    /// A place for the glyph inside a run of words, drawn by `text`.
    static func mark(size: CGFloat) -> AttributedString {
        var mark = AttributedString("\u{FFFC}")
        mark[FileGlyph.self] = size
        return mark
    }

    /// The words as one `Text`, with each glyph's place drawn as the file icon.
    @MainActor static func text(_ words: AttributedString, ink: Color) -> Text {
        guard words.runs.contains(where: { $0[FileGlyph.self] != nil }) else { return Text(words) }
        var out = Text("")
        for run in words.runs {
            if let size = run[FileGlyph.self], let image = image(size) {
                out =
                    out + Text(Image(uiImage: image).renderingMode(.template)).foregroundColor(ink).baselineOffset(-1.5)
            } else {
                out = out + Text(AttributedString(words[run.range]))
            }
        }
        return out
    }

    @MainActor private static var drawn: [CGFloat: UIImage] = [:]

    @MainActor private static func image(_ size: CGFloat) -> UIImage? {
        if let found = drawn[size] { return found }
        let renderer = ImageRenderer(content: IconView(icon: Icons.file, size: size).foregroundStyle(.black))
        renderer.scale = UIScreen.main.scale
        guard let image = renderer.uiImage?.withRenderingMode(.alwaysTemplate) else { return nil }
        drawn[size] = image
        return image
    }
}
