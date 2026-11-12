import SwiftUI

/// Patches as the transcript draws them.
enum DiffText {
    /// The lines a change added and removed, as "+3 −1" in the git colours.
    static func counts(adds: Int, dels: Int, _ palette: Palette) -> Text {
        let added = Text("+\(adds)").foregroundStyle(palette.gitAdded)
        let deleted = Text("−\(dels)").foregroundStyle(palette.gitDeleted)
        return Text("\(added) \(deleted)")
    }

    private static func lines(_ text: String) -> [String] {
        guard !text.isEmpty else { return [] }
        var lines = text.components(separatedBy: "\n")
        if lines.count > 1, lines.last == "" { lines.removeLast() }
        return lines
    }

    /// A fenced block the agent wrote is sometimes a patch; its signs are worth colouring and its changed words marking.
    static func fenced(_ text: String, _ language: String?) -> [DiffLine]? {
        let raw = lines(text)
        guard raw.count >= 2, raw.count <= 400 else { return nil }
        let signed = raw.filter { $0.hasPrefix("+") || $0.hasPrefix("-") }
        if language != "diff" && language != "patch" {
            let shaped = raw.filter { $0.isEmpty || "-+@ ".contains($0.first!) }
            let adds = raw.contains { $0.hasPrefix("+") }
            let dels = raw.contains { $0.hasPrefix("-") }
            if !adds || !dels || signed.count < 2 || Double(shaped.count) < Double(raw.count) * 0.8 { return nil }
        }
        guard !signed.isEmpty else { return nil }
        var out = raw.map { line -> DiffLine in
            let sign = line.hasPrefix("+") ? "+" : line.hasPrefix("-") ? "-" : " "
            let text = line.first.map { "-+ ".contains($0) } == true ? String(line.dropFirst()) : line
            return DiffLine(sign: sign, text: text)
        }
        markPairs(&out)
        return out
    }

    private static func markPairs(_ lines: inout [DiffLine]) {
        guard lines.count > 1 else { return }
        for index in 0..<(lines.count - 1) {
            guard lines[index].sign == "-", lines[index + 1].sign == "+" else { continue }
            if index > 0, lines[index - 1].sign == "-" { continue }
            if index + 2 < lines.count, lines[index + 2].sign == "+" { continue }
            let a = Array(lines[index].text.utf16)
            let b = Array(lines[index + 1].text.utf16)
            guard a.count <= 200, b.count <= 200 else { continue }
            var start = 0
            while start < a.count, start < b.count, a[start] == b[start] { start += 1 }
            var end = 0
            while end < a.count - start, end < b.count - start, a[a.count - 1 - end] == b[b.count - 1 - end] {
                end += 1
            }
            let shared = start + end
            if shared < 3 || Double(shared) < Double(min(a.count, b.count)) * 0.25 { continue }
            lines[index].mark = [start, a.count - end]
            lines[index + 1].mark = [start, b.count - end]
        }
    }

    enum Row {
        case line(DiffLine)
        case gap(Int)
    }

    /// Only the lines around a change are worth the room; the rest fold into a gap the reader can open.
    static func collapse(_ lines: [DiffLine], context: Int) -> [Row] {
        var keep = [Bool](repeating: false, count: lines.count)
        for (index, line) in lines.enumerated() where line.sign != " " {
            for near in max(0, index - context)...min(lines.count - 1, index + context) { keep[near] = true }
        }
        var rows: [Row] = []
        var run = 0
        for (index, line) in lines.enumerated() {
            if keep[index] {
                if run > 0 { rows.append(.gap(run)) }
                run = 0
                rows.append(.line(line))
            } else {
                run += 1
            }
        }
        if run > 0 { rows.append(.gap(run)) }
        return rows
    }
}

/// The two number columns and the sign's, before a diff line's text, as on the Mac but narrower.
private let numberWidth: CGFloat = 28
private let signWidth: CGFloat = 14

/// A patch's inks, mixed once per box rather than once per line.
struct DiffInks {
    let number: Color
    let addedNumber: Color
    let deletedNumber: Color
    let addedText: Color
    let deletedText: Color

    init(_ palette: Palette) {
        number = palette.inkFaint.mixed(with: palette.sunken, 0.5)
        addedNumber = palette.gitAdded.mixed(with: palette.sunken, 0.38)
        deletedNumber = palette.gitDeleted.mixed(with: palette.sunken, 0.38)
        addedText = palette.gitAdded.mixed(with: palette.ink, 0.22)
        deletedText = palette.gitDeleted.mixed(with: palette.ink, 0.25)
    }
}

/// Rows of a patch scrolling sideways together, so long lines keep to the screen's width and each row's tint spans
/// the longest.
struct DiffRows: View {
    @Environment(\.palette) private var palette
    let lines: [DiffLine]
    let numbered: Bool
    var onGap: (() -> Void)?
    var rows: [DiffText.Row]?
    /// The box's own width, which every row's tint spans at least.
    @State private var width: CGFloat = 0

    var body: some View {
        let inks = DiffInks(palette)
        ScrollView(.horizontal, showsIndicators: false) {
            VStack(alignment: .leading, spacing: 0) {
                ForEach(Array((rows ?? lines.map(DiffText.Row.line)).enumerated()), id: \.offset) { _, row in
                    switch row {
                    case .line(let line): DiffRow(line: line, numbered: numbered, inks: inks)
                    case .gap(let count):
                        Button {
                            onGap?()
                        } label: {
                            HStack(spacing: 6) {
                                IconView(icon: Icons.chevron, size: 9).foregroundStyle(palette.inkDim)
                                Text(count == 1 ? LocalizedStringKey("1 more line") : "\(count) more lines")
                                    .font(Theme.ui(11))
                                    .foregroundStyle(palette.inkDim)
                            }
                            .frame(height: 26)
                            .padding(.leading, numberWidth * 2 + signWidth)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
            .frame(minWidth: width, alignment: .leading)
            .padding(.vertical, 4)
        }
        .onGeometryChange(for: CGFloat.self) {
            $0.size.width
        } action: {
            width = $0
        }
    }
}

/// One line of a patch: old and new numbers, a +/− gutter and the text tinted by what happened to it.
struct DiffRow: View {
    @Environment(\.palette) private var palette
    let line: DiffLine
    let numbered: Bool
    let inks: DiffInks

    var body: some View {
        let added = line.sign == "+"
        let deleted = line.sign == "-"
        let numberInk = added ? inks.addedNumber : deleted ? inks.deletedNumber : inks.number
        HStack(alignment: .top, spacing: 0) {
            if numbered {
                number(line.oldLine, numberInk)
                number(line.newLine, numberInk)
            }
            Text(added ? "+" : deleted ? "−" : "")
                .font(Theme.mono(11))
                .foregroundStyle(added ? palette.gitAdded : deleted ? palette.gitDeleted : palette.inkFaint)
                .frame(width: signWidth, alignment: .center)
            Text(marked(added: added, deleted: deleted))
                .font(Theme.mono(11))
                .foregroundStyle(added ? inks.addedText : deleted ? inks.deletedText : palette.inkDim)
                .fixedSize()
        }
        .lineSpacing(20 - 11 * 1.32)
        .padding(.leading, numbered ? 0 : 4)
        .padding(.trailing, 10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(added ? palette.gitAdded.opacity(0.09) : deleted ? palette.gitDeleted.opacity(0.09) : .clear)
    }

    private func number(_ value: Int?, _ ink: Color) -> some View {
        Text(value.map(String.init) ?? "")
            .font(Theme.mono(10))
            .foregroundStyle(ink)
            .frame(width: numberWidth - 6, alignment: .trailing)
            .padding(.trailing, 6)
    }

    private func marked(added: Bool, deleted: Bool) -> AttributedString {
        let units = Array(line.text.utf16)
        guard let mark = line.mark, mark.count == 2, mark[0] <= mark[1], mark[1] <= units.count else {
            return AttributedString(line.text)
        }
        let piece = { (from: Int, to: Int) in String(decoding: units[from..<to], as: UTF16.self) }
        var middle = AttributedString(piece(mark[0], mark[1]))
        middle.backgroundColor = (deleted ? palette.gitDeleted : palette.gitAdded).opacity(0.26)
        return AttributedString(piece(0, mark[0])) + middle + AttributedString(piece(mark[1], units.count))
    }
}

/// What an edit changed, as the Mac draws it under the call: old and new line numbers, a +/− gutter, tinted text and
/// the unchanged runs folded. `waiting` stands in for the lines until the host has sent them.
struct DiffBody: View {
    @Environment(\.palette) private var palette
    let lines: [DiffLine]
    let fold: String
    var waiting: String?
    let folds: Folds

    /// Unchanged lines kept around each change, as on the Mac.
    private static let context = 3
    /// Rows drawn before the rest waits for a tap, so opening a whole new file costs one screenful.
    private static let firstRows = 120

    var body: some View {
        let expanded = folds.isOpen("\(fold)/context", false)
        let whole = folds.isOpen("\(fold)/rows", false)
        let rows = DiffText.collapse(lines, context: expanded ? Int.max / 2 : Self.context)
        let shown = whole ? rows : Array(rows.prefix(Self.firstRows))
        let more = rows.count - shown.count
        VStack(alignment: .leading, spacing: 0) {
            if let waiting {
                Text(waiting).font(Theme.mono(10.5)).foregroundStyle(palette.inkFaint).padding(.horizontal, 10)
                    .padding(.vertical, 7)
            }
            if !shown.isEmpty {
                DiffRows(lines: [], numbered: true, onGap: { folds.set("\(fold)/context", true) }, rows: shown)
            }
            if more > 0 {
                Button {
                    folds.set("\(fold)/rows", true)
                } label: {
                    Text("Show \(more) more lines")
                        .font(Theme.ui(11))
                        .foregroundStyle(palette.inkDim)
                        .padding(.horizontal, 10)
                        .padding(.vertical, 7)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(palette.sunken)
        .clipShape(RoundedRectangle(cornerRadius: 8))
    }
}
