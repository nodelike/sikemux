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
    /// An agent's reply, set a touch looser than other prose.
    static func reply(_ palette: Palette) -> ProseStyle { ProseStyle(lineHeight: 24.5, color: palette.ink) }
    static func thought(_ palette: Palette) -> ProseStyle {
        ProseStyle(size: 14.5, lineHeight: 24.5, color: palette.secondary)
    }
}

/// Links the transcript opens itself rather than handing to the system: files on the host.
enum TranscriptLink {
    static let fileScheme = "sikemux-file"

    /// A link to the host's file `raw`, only for a picture, the one kind of file the phone opens.
    static func file(_ raw: String) -> URL? {
        guard HostFiles.imagePath(raw) != nil else { return nil }
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
            run.markFound(found, current: marks.current == range.lowerBound, palette)
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
        case .faint(let faint):
            var run = string(faint.c)
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
            guard paths, HostFiles.namesFile(path.href) else { return label }
            var run = FileGlyph.mark(size: 12) + AttributedString(" ") + label
            run.link = TranscriptLink.file(path.href)
            if path.code != true { run.foregroundColor = palette.accent }
            return run
        case .img(let image):
            if let url = TranscriptLink.openable(image.src) {
                return linked(plain(image.alt.isEmpty ? image.src : image.alt, counts: false), url)
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
                return linked(label, url)
            } else if link.file == true, paths {
                label = FileGlyph.mark(size: 12) + AttributedString(" ") + label
                label.link = TranscriptLink.file(link.href)
                label.foregroundColor = palette.accent
            }
            return label
        default:
            return element.children.map(string) ?? AttributedString()
        }
    }

    /// A web link as the Mac's reply draws it: lavender and underlined, after the mark of the service it leads to.
    private func linked(_ label: AttributedString, _ url: URL) -> AttributedString {
        let ink = palette.accent.mixed(with: palette.ink, 0.5)
        var words = label
        words.foregroundColor = ink
        words.underlineStyle = Text.LineStyle(pattern: .solid, color: ink.opacity(0.4))
        if let mark = ServiceMark.of(url) {
            words = FileGlyph.mark(icon: mark, size: 12, tint: ink) + AttributedString(" ") + words
        }
        words.link = url
        return words
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
    nodes.map { FindText.words($0, separated: false) }.joined()
}

func visibleText(_ element: MdElement) -> String {
    FindText.words(.element(element), separated: false)
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
        VStack(alignment: .leading, spacing: 12) {
            ForEach(Array(blocks.enumerated()), id: \.offset) { index, block in
                BlockView(
                    block: block, style: style, palette: palette, marks: marks?.from(starts![index]),
                    fading: index == blocks.count - 1 ? fading : [], first: index == 0,
                    lead: index > 0 && Self.isTitle(blocks[index - 1]) && Self.isParagraph(block)
                ).equatable()
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private static func isTitle(_ block: MdElement) -> Bool {
        if case .h(let heading) = block { heading.l == 1 } else { false }
    }

    private static func isParagraph(_ block: MdElement) -> Bool {
        if case .p = block { true } else { false }
    }
}

/// Headings as the Mac's editorial reply sets them, against the text's size: ### reads as a spaced-capital label.
private let headingScales: [CGFloat] = [1.52, 1.24, 0.72]
/// The room above a heading that follows other blocks, against the text's size.
private let headingRoom: [CGFloat] = [0.61, 1.5, 0.4]

struct BlockView: View, Equatable {
    let block: MdElement
    let style: ProseStyle
    let palette: Palette
    var marks: Marks?
    var fading: Fading = []
    /// Nothing stands above it, so a heading needs no room over it.
    var first = true
    /// The paragraph straight under the text's title, set a touch larger.
    var lead = false
    /// How deep a list sits inside other lists; only the outermost one's bullets take the accent.
    var depth = 0

    nonisolated static func == (a: BlockView, b: BlockView) -> Bool {
        a.block == b.block && a.style == b.style && a.palette == b.palette && a.marks == b.marks
            && a.fading.map(\.text) == b.fading.map(\.text) && a.fading.map(\.opacity) == b.fading.map(\.opacity)
            && a.first == b.first && a.lead == b.lead && a.depth == b.depth
    }

    private var inline: Inline {
        Inline(palette: palette, style: style, marks: marks, cursor: marks.map { FindCursor($0.base) })
    }

    var body: some View {
        switch block {
        case .pre(let pre):
            CodeBlock(lang: pre.lang ?? "", name: pre.name, text: pre.v)
        case .h(let heading):
            HeadingBlock(heading: heading, style: style, palette: palette, marks: marks, fading: fading, first: first)
        case .ul(let list):
            ListBlock(items: list.c, first: nil, style: style, palette: palette, marks: marks, depth: depth)
        case .ol(let list):
            ListBlock(items: list.c, first: list.start ?? 1, style: style, palette: palette, marks: marks, depth: depth)
        case .quote(let quote):
            let quoted = ProseStyle(size: style.size, lineHeight: style.lineHeight, color: style.color, italic: true)
            Contents(nodes: quote.c, style: quoted, palette: palette, marks: marks, separated: true, depth: depth)
                .padding(.leading, 16)
                .overlay(alignment: .leading) { Rectangle().fill(palette.accent).frame(width: 2) }
        case .hr:
            Hairline(color: palette.borderStrong).padding(.vertical, 4)
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
            let set =
                lead ? ProseStyle(size: style.size * 1.07, lineHeight: style.lineHeight, color: style.color) : style
            Prose(
                text: Inline(palette: palette, style: set, marks: marks, cursor: marks.map { FindCursor($0.base) })
                    .string(paragraph.c).fading(fading, color: set.color), style: set)
        default:
            Prose(text: inline.string(block), style: style)
        }
    }
}

/// A heading at its level's size against the text's: ## stands larger with room above, ### is a spaced-capital label.
private struct HeadingBlock: View {
    let heading: MdElement.H
    let style: ProseStyle
    let palette: Palette
    var marks: Marks?
    var fading: Fading
    let first: Bool

    var body: some View {
        let level = heading.l
        let label = level == 3
        let scale = headingScales[safe: level - 1]
        let size = scale.map { style.size * $0 } ?? style.size
        let set = ProseStyle(
            size: size, lineHeight: scale.map { _ in size * (label ? 1.6 : 1.3) } ?? style.lineHeight,
            color: label ? palette.inkDim : palette.ink)
        let tracking: CGFloat = level == 1 ? -0.33 : level == 2 ? -0.18 : label ? 1.67 : 0
        let words =
            label
            ? AttributedString(visibleText(heading.c).uppercased())
            : Inline(palette: palette, style: set, marks: marks, cursor: marks.map { FindCursor($0.base) })
                .string(heading.c).fading(fading, color: set.color)
        Prose(
            text: words.settingAttributes(AttributeContainer().font(Theme.ui(size, .semibold)).tracking(tracking)),
            style: set
        )
        .padding(.top, first ? 0 : style.size * (headingRoom[safe: level - 1] ?? 0.4))
        .accessibilityAddTraits(.isHeader)
    }
}

/// Words set in the transcript's prose, selectable as the Mac's are.
struct Prose: View {
    @Environment(\.palette) private var palette
    @Environment(\.displayScale) private var scale
    let text: AttributedString
    let style: ProseStyle

    var body: some View {
        FileGlyph.text(text, ink: palette.inkDim, scale: scale)
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
    var depth = 0

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
                    BlockView(block: block, style: style, palette: palette, marks: marks?.from(start), depth: depth)
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
    let depth: Int

    var body: some View {
        let starts = marks.map { FindText.starts(items, separated: true, top: false, at: $0.base) } ?? []
        VStack(alignment: .leading, spacing: 6) {
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
                        } else if let first {
                            Text(String(format: "%02d", first + index))
                                .font(Theme.mono(11))
                                .foregroundStyle(palette.accent)
                                .frame(minWidth: 20, alignment: .leading)
                        } else {
                            Text("•")
                                .font(style.font)
                                .foregroundStyle(depth > 0 ? palette.inkDim : palette.accent)
                                .padding(.leading, 4)
                                .frame(minWidth: 14, alignment: .leading)
                        }
                        Contents(
                            nodes: item.c,
                            style: item.checked == true
                                ? ProseStyle(
                                    size: style.size, lineHeight: style.lineHeight, color: palette.tertiary,
                                    italic: style.italic) : style, palette: palette,
                            marks: starts.isEmpty ? nil : marks?.from(starts[index]), depth: depth + 1)
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
                    (details.summary.isEmpty ? Text("Details") : Text(verbatim: details.summary))
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

/// A fence as the Mac draws it: one card headed by the file it shows, its folder quiet beside it, and a Copy pill,
/// then the code with its lines numbered, or a patch's rows tinted by sign with the changed words marked.
struct CodeBlock: View, Equatable {
    @Environment(\.palette) private var palette
    let lang: String
    /// The file the block shows, when its fence names one.
    let name: String?
    let text: String

    nonisolated static func == (a: CodeBlock, b: CodeBlock) -> Bool {
        a.lang == b.lang && a.name == b.name && a.text == b.text
    }

    private static let shells: Set = [
        "console", "terminal", "shell-session", "shellsession", "sh-session", "fish", "nu", "ksh", "csh", "tcsh",
    ]
    private static let commands: Set = ["sh", "bash", "zsh", "shell"]

    /// What a block without a file name is called, and whether its lines are numbered: commands to type are a Terminal.
    private static func title(_ lang: String, _ code: String) -> (title: String, numbered: Bool) {
        let lower = lang.lowercased()
        let typed = lang.isEmpty ? code.hasPrefix("$ ") : shells.contains(lower) || commands.contains(lower)
        return typed ? (String(localized: "Terminal"), false) : (lang.isEmpty ? String(localized: "Code") : lang, true)
    }

    var body: some View {
        let code = text.hasSuffix("\n") ? String(text.dropLast()) : text
        let named =
            name.map { (title: $0, numbered: true) } ?? Self.title(lang.removingPercentEncoding ?? lang, code)
        let cut = name?.lastIndex(of: "/").flatMap { $0 > named.title.startIndex ? $0 : nil }
        let folder = cut.map { String(named.title[..<$0]) }
        let file = cut.map { String(named.title[named.title.index(after: $0)...]) } ?? named.title
        let lines =
            named.numbered && code.contains("\n")
            ? code.split(separator: "\n", omittingEmptySubsequences: false).count : 0
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                IconView(icon: named.numbered ? Icons.file : Icons.terminal, size: 13).foregroundStyle(palette.inkFaint)
                Text(file).font(Theme.ui(12, .medium)).foregroundStyle(palette.ink).lineLimit(1)
                    .layoutPriority(1)
                if let folder {
                    Text(folder).font(Theme.ui(11.5)).foregroundStyle(palette.inkFaint).lineLimit(1)
                        .truncationMode(.head)
                }
                Spacer(minLength: 0)
                CopyButton(value: code, label: String(localized: "code"), worded: true)
            }
            .padding(.leading, 14)
            .padding(.trailing, 8)
            .frame(height: 38)
            if let patch = DiffText.fenced(text, lang.isEmpty ? nil : lang) {
                DiffRows(lines: patch, numbered: false)
            } else {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(alignment: .top, spacing: 16) {
                        if lines > 0 {
                            Text((1...lines).map { String($0) }.joined(separator: "\n"))
                                .multilineTextAlignment(.trailing)
                                .foregroundStyle(palette.inkFaint.mixed(with: palette.raised, 0.5))
                                .frame(width: 24, alignment: .trailing)
                        }
                        Text(code).foregroundStyle(palette.ink).fixedSize()
                    }
                    .font(Theme.mono(12))
                    .lineSpacing(20 - 12 * 1.32)
                    .padding(.leading, lines > 0 ? 0 : 16)
                    .padding(.trailing, 16)
                    .padding(.bottom, 14)
                }
            }
        }
        .background(palette.raised)
        .clipShape(RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(palette.border, lineWidth: 1))
    }
}

/// The marks the Mac puts before a link to these services, from its own icons.
enum ServiceMark {
    private static let services: [([String], String)] = [
        (["github.com"], "IconGithub"),
        (["bitbucket.org"], "IconBitbucket"),
        (["atlassian.net"], "IconJira"),
        (["gitlab.com"], "IconGitlab"),
        (["codeberg.org"], "IconForgejo"),
        (["gitea.com"], "IconGitea"),
        (["linear.app"], "IconLinear"),
        (["vercel.com", "vercel.app"], "IconVercel"),
        (["slack.com"], "IconSlack"),
        (["aws.amazon.com", "amazonaws.com"], "IconAws"),
        (["grok.com"], "IconGrok"),
        (["apple.com"], "IconApple"),
        (["play.google.com"], "IconGoogleplay"),
    ]

    /// The name of the mark of the service a web link leads to.
    static func of(_ url: URL) -> String? {
        guard let scheme = url.scheme?.lowercased(), scheme == "http" || scheme == "https",
            let host = url.host?.lowercased()
        else { return nil }
        return services.first { service in service.0.contains { host == $0 || host.hasSuffix(".\($0)") } }?.1
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
            CopyButton(value: tableSource(table), label: String(localized: "table"), size: 12).background(
                palette.raised
            ).padding(3)
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

/// The glyph a file or a service the transcript names wears before its words, as the Mac's file refs and links do.
enum FileGlyph: AttributedStringKey {
    struct Glyph: Hashable, Sendable {
        /// An icon's name, as `Icons.named` takes it.
        let icon: String
        let size: CGFloat
        /// The colour drawn into a mark that keeps its own; nil takes the line's ink.
        let tint: Color?
    }

    typealias Value = Glyph
    static let name = "sikemux.fileGlyph"

    /// A place for the file glyph inside a run of words, drawn by `text`.
    static func mark(size: CGFloat) -> AttributedString {
        mark(icon: "IconFile", size: size, tint: nil)
    }

    /// A place for the icon `icon` inside a run of words, drawn in its own colours with `tint` for the rest.
    static func mark(icon: String, size: CGFloat, tint: Color?) -> AttributedString {
        var mark = AttributedString("\u{FFFC}")
        mark[FileGlyph.self] = Glyph(icon: icon, size: size, tint: tint)
        return mark
    }

    /// The words as one `Text`, with each glyph's place drawn as its icon.
    @MainActor static func text(_ words: AttributedString, ink: Color, scale: CGFloat) -> Text {
        guard words.runs.contains(where: { $0[FileGlyph.self] != nil }) else { return Text(words) }
        var out = Text("")
        for run in words.runs {
            if let glyph = run[FileGlyph.self], let image = image(glyph, scale: scale) {
                let drawn =
                    glyph.tint == nil
                    ? Text(Image(uiImage: image).renderingMode(.template)).foregroundStyle(ink)
                    : Text(Image(uiImage: image).renderingMode(.original))
                out = Text("\(out)\(drawn.baselineOffset(-1.5))")
            } else {
                out = Text("\(out)\(Text(AttributedString(words[run.range])))")
            }
        }
        return out
    }

    private struct Drawn: Hashable {
        let glyph: Glyph
        let scale: CGFloat
    }

    @MainActor private static var drawn: [Drawn: UIImage] = [:]

    @MainActor private static func image(_ glyph: Glyph, scale: CGFloat) -> UIImage? {
        let key = Drawn(glyph: glyph, scale: scale)
        if let found = drawn[key] { return found }
        guard let icon = Icons.named(glyph.icon) else { return nil }
        let renderer = ImageRenderer(
            content: IconView(icon: icon, size: glyph.size).foregroundStyle(glyph.tint ?? .black))
        renderer.scale = scale
        guard let image = renderer.uiImage else { return nil }
        let shown = glyph.tint == nil ? image.withRenderingMode(.alwaysTemplate) : image
        drawn[key] = shown
        return shown
    }
}
