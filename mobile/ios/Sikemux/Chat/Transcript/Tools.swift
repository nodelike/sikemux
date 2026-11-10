import SwiftUI

typealias ToolPart = ChatPart.Tool

/// Groups a message's parts: consecutive calls share one group, except a page or GIF the agent showed, which draws in
/// the call's place and is part of its reply.
enum PartGroup: Identifiable {
    case tools(String, [ToolPart])
    case part(ChatPart)

    var id: String {
        switch self {
        case .tools(let id, _): id
        case .part(let part): part.id
        }
    }

    static func of(_ parts: [ChatPart]) -> [PartGroup] {
        var groups: [PartGroup] = []
        for part in parts {
            if case .tool(let tool) = part, tool.page == nil, tool.gif == nil {
                if case .tools(let id, var tools) = groups.last {
                    tools.append(tool)
                    groups[groups.count - 1] = .tools(id, tools)
                } else {
                    groups.append(.tools(tool.id, [tool]))
                }
            } else {
                groups.append(.part(part))
            }
        }
        return groups
    }
}

private let toolIcons: [String: String] = [
    "activity": "IconActivity", "agent": "IconAgent", "book": "IconBook", "camera": "IconCamera", "click": "IconClick",
    "clock": "IconClock", "close": "IconClose", "code": "IconCode", "command": "IconCommand", "dashboard": "IconDashboard",
    "download": "IconDownload", "eye": "IconEye", "file": "IconFile", "folder": "IconFolder", "globe": "IconGlobe",
    "highlighter": "IconHighlighter", "home-button": "IconHomeButton", "hourglass": "IconHourglass", "issue": "IconIssue",
    "keyboard": "IconKeyboard", "link": "IconLink", "log-lines": "IconLogLines", "logo": "Logo", "message": "IconMessage",
    "mouse": "IconMouse", "move": "IconMove", "network": "IconNetwork", "panel": "IconPanel", "pencil": "IconPencil",
    "phone": "IconPhone", "pinch": "IconPinch", "plug": "IconPlug", "pointer": "IconPointer",
    "pull-request": "IconPullRequest", "record": "IconRecord", "refresh": "IconRefresh", "resize": "IconResize",
    "rocket": "IconRocket", "rotate": "IconRotate", "run": "IconRun", "search": "IconSearch",
    "square-plus": "IconSquarePlus", "status": "IconStatus", "steps": "IconSteps", "stop": "IconStop",
    "swipe": "IconSwipe", "tabs": "IconTabs", "tap": "IconTap", "terminal": "IconTerminal", "text": "IconText",
    "touch-path": "IconTouchPath", "trash": "IconTrash", "unlink": "IconUnlink", "upload": "IconUpload",
    "warning": "IconWarning", "waterfall": "IconWaterfall", "window": "IconWindow", "workflow": "IconWorkflow",
    "wrench": "IconWrench",
]

private let actionIcons: [ToolAction: String] = [
    .mcp: "plug", .read: "file", .search: "search", .edit: "pencil", .move: "move", .delete: "trash",
    .execute: "command", .fetch: "globe",
]

/// The marks of the plugins whose tools the phone's icon set already draws; the rest keep the call's own icon.
private let pluginMarks: [String: String] = ["sikemux.aws": "IconAws", "sikemux.rundeck": "IconRundeck"]

/// The icon and colour of a call, as the Mac's tool rows draw them.
private func look(_ tool: ToolCall, _ palette: Palette) -> (IconDef, Color) {
    let failed = tool.status == .failed
    let name = failed ? "warning" : tool.row.icon ?? actionIcons[tool.row.action] ?? "agent"
    let plugin = failed ? nil : tool.row.plugin.flatMap { pluginMarks[$0] }
    let icon = Icons.named(plugin ?? toolIcons[name] ?? "IconAgent") ?? Icons.agentLine
    if failed { return (icon, palette.danger) }
    let color: Color = switch tool.row.action {
    case .read, .search, .fetch: palette.toolRead
    case .edit, .move: palette.toolEdit
    case .delete: palette.toolDelete
    case .execute: palette.toolRun
    case .mcp: palette.toolMcp
    default: palette.inkDim
    }
    return (icon, color)
}

/// Counts up in whole seconds, so the tick redraws this label and nothing around it.
struct LiveSeconds: View {
    let since: Date
    var spent: Double = 0
    let font: Font
    let color: Color

    var body: some View {
        TimelineView(.periodic(from: since, by: 1)) { context in
            Text(ChatWords.elapsed(seconds: max(0, Int((spent / 1000 + context.date.timeIntervalSince(since)).rounded(.down)))))
                .font(font)
                .foregroundStyle(color)
                .monospacedDigit()
        }
    }
}

extension ToolPart {
    var startDate: Date? { startedAt.map { Date(timeIntervalSince1970: Double($0) / 1000) } }

    /// A call the turn cut off has a duration, but printing it would read as a call that ran that long and then
    /// finished, so it says why it stopped.
    var elapsed: String? {
        if tool.status == .cancelled { return "stopped" }
        guard let startedAt, let endedAt else { return nil }
        return ChatWords.duration(ms: Double(endedAt - startedAt))
    }
}

/// A run of calls stays open while the agent is still adding to it, as on the Mac, and folds once something follows
/// it or the turn ends. While a call runs, the header says what the agent said it is for.
struct ToolGroup: View {
    @Environment(\.palette) private var palette
    @Environment(TranscriptContext.self) private var context
    let id: String
    let parts: [ToolPart]
    let live: Bool

    var body: some View {
        let current = parts.first { $0.tool.running }
        let said = current?.tool.description
        let spent = parts.reduce(0.0) { total, part in
            guard let startedAt = part.startedAt, let endedAt = part.endedAt else { return total }
            return total + Double(endedAt - startedAt)
        }
        let kindWidth = CGFloat(min(16, max(4, parts.map { ChatWords.toolKind($0.tool).count }.max() ?? 4))) * 6.9 + 1
        let open = context.folds.isOpen(id, live || current != nil)
        let calls = ChatWords.calls(parts.count)
        VStack(alignment: .leading, spacing: 0) {
            Button {
                context.folds.set(id, !open)
            } label: {
                HStack(spacing: 8) {
                    if current != nil {
                        Shimmer(text: said ?? calls, font: Theme.ui(12.5), color: said != nil ? palette.secondary : palette.inkDim)
                    } else {
                        Text(calls).font(Theme.ui(12.5)).foregroundStyle(palette.inkDim).lineLimit(1)
                    }
                    if said != nil, parts.count > 1 {
                        Text(ChatWords.callCount(parts.count)).font(Theme.mono(11)).foregroundStyle(palette.inkDim)
                    }
                    if let current {
                        LiveSeconds(since: current.startDate ?? Date(), spent: spent, font: Theme.mono(11), color: palette.inkDim)
                            .id(current.id)
                    } else if spent > 0 {
                        Text(ChatWords.duration(ms: spent)).font(Theme.mono(11)).foregroundStyle(palette.inkDim)
                    }
                    IconView(icon: Icons.chevron, size: 11)
                        .foregroundStyle(palette.inkDim)
                        .opacity(0.7)
                        .rotationEffect(.degrees(open ? 90 : 0))
                }
                .frame(minHeight: 24)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("\(said ?? calls)\(current != nil ? ", running" : "")")
            .accessibilityValue(open ? "expanded" : "collapsed")
            if open {
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(Array(parts.enumerated()), id: \.element.id) { index, part in
                        ToolRowView(part: part, last: index == parts.count - 1, group: id, kindWidth: kindWidth)
                    }
                }
                .padding(.leading, 6)
            }
        }
        .padding(.vertical, 6)
    }
}

private struct ToolRowView: View {
    @Environment(\.palette) private var palette
    @Environment(TranscriptContext.self) private var context
    let part: ToolPart
    let last: Bool
    let group: String
    let kindWidth: CGFloat

    var body: some View {
        let tool = part.tool
        let (icon, color) = look(tool, palette)
        let running = tool.running
        let command = tool.kind == "execute" ? tool.title.trimmingCharacters(in: .whitespacesAndNewlines) : nil
        // The row holds one line of a command, so the whole of it is worth opening while it runs or when the row
        // shortened it.
        let opens = part.diff != nil || part.output != nil || part.failure != nil || (command != nil && (running || command != tool.row.target))
        let open = context.folds.isOpen("\(group)/\(part.id)", false)
        VStack(alignment: .leading, spacing: 0) {
            Button {
                context.folds.set("\(group)/\(part.id)", !open)
            } label: {
                HStack(spacing: 8) {
                    Rectangle().fill(palette.treeTick).frame(width: 10, height: 1).opacity(last && !open ? 0 : 1)
                    IconView(icon: icon, size: 12).foregroundStyle(color)
                    Text(ChatWords.toolKind(tool)).font(Theme.mono(11.5)).foregroundStyle(color).lineLimit(1)
                        .frame(minWidth: kindWidth, alignment: .leading)
                    if running {
                        Shimmer(text: tool.row.target + (tool.row.detail.map { " \($0)" } ?? ""), font: Theme.mono(11.5), color: palette.inkDim)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    } else {
                        target(tool)
                            .font(Theme.mono(11.5))
                            .lineLimit(1)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    HStack(spacing: 6) {
                        if let diff = part.diff {
                            (Text("+\(diff.adds)").foregroundColor(palette.gitAdded) + Text(" ") + Text("−\(diff.dels)").foregroundColor(palette.gitDeleted))
                                .font(Theme.mono(10.5))
                        }
                        if running {
                            LiveSeconds(since: part.startDate ?? Date(), font: Theme.mono(10.5), color: palette.inkDim)
                        } else if let elapsed = part.elapsed {
                            Text(elapsed).font(Theme.mono(10.5)).foregroundStyle(palette.inkDim)
                        }
                        if opens {
                            IconView(icon: Icons.chevron, size: 10)
                                .foregroundStyle(palette.inkDim)
                                .opacity(open ? 1 : 0.55)
                                .rotationEffect(.degrees(open ? 90 : 0))
                        }
                    }
                }
                .frame(minHeight: 22)
                .background(alignment: .leading) { Spine(last: last && !open) }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .disabled(!opens)
            .accessibilityLabel("\(ChatWords.toolKind(tool)) \(tool.row.target)\(tool.status == .failed ? ", failed" : running ? ", running" : "")")
            if open {
                ToolDetailView(part: part, command: command, group: group)
                    .padding(.leading, 18)
                    .padding(.top, 2)
                    .padding(.bottom, 6)
                    .background(alignment: .leading) { if !last { Rectangle().fill(palette.treeSpine).frame(width: 1) } }
            }
        }
        .opacity(tool.status == .cancelled ? 0.6 : 1)
    }

    private func target(_ tool: ToolCall) -> Text {
        let target = tool.row.target
        var words = AttributedString(target)
        words.foregroundColor = palette.ink
        if let path = tool.row.path, let url = TranscriptLink.file(path) {
            words.link = url
        } else if let range = target.range(of: #"https?://[^\s<>"'`]+"#, options: .regularExpression) {
            var raw = String(target[range])
            while let last = raw.last, ".,;:!?)]".contains(last) { raw.removeLast() }
            if let url = TranscriptLink.openable(raw), let found = words.range(of: raw) {
                words[found].link = url
                words[found].foregroundColor = palette.accent
            }
        }
        var text = Text(words)
        if let detail = tool.row.detail { text = text + Text(" \(detail)").foregroundColor(palette.inkDim) }
        return text
    }
}

/// The line down a run of calls; the last row's bends into its tick, ending on the row's middle like the Mac's.
private struct Spine: View {
    @Environment(\.palette) private var palette
    let last: Bool

    var body: some View {
        GeometryReader { size in
            if last {
                Path { path in
                    path.move(to: CGPoint(x: 0.5, y: 0))
                    path.addLine(to: CGPoint(x: 0.5, y: size.size.height / 2 - 4))
                    path.addQuadCurve(to: CGPoint(x: 4.5, y: size.size.height / 2), control: CGPoint(x: 0.5, y: size.size.height / 2))
                    path.addLine(to: CGPoint(x: 10, y: size.size.height / 2))
                }
                .stroke(palette.treeSpine, lineWidth: 1)
            } else {
                Rectangle().fill(palette.treeSpine).frame(width: 1)
            }
        }
    }
}

/// What an opened row shows beyond its summary, read from the host when the row opens and again as a running call
/// goes on.
private struct ToolDetailView: View {
    @Environment(\.palette) private var palette
    @Environment(TranscriptContext.self) private var context
    let part: ToolPart
    let command: String?
    let group: String
    @State private var detail: ToolDetail?
    @State private var problem: String?

    var body: some View {
        let waiting = detail != nil ? nil : problem.map { "Could not load it: \($0)" } ?? "Loading…"
        Group {
            if let diff = part.diff {
                DiffBody(
                    path: detail?.diff?.path ?? diff.path, adds: detail?.diff?.adds ?? diff.adds, dels: detail?.diff?.dels ?? diff.dels,
                    lines: detail?.diff?.lines ?? [], fold: "\(group)/\(part.id)/diff",
                    waiting: detail?.diff != nil ? nil : waiting, folds: context.folds)
            } else if command != nil || part.output != nil {
                Terminal(part: part, command: command, group: group, output: detail?.output, waiting: part.output != nil ? waiting : nil)
            } else {
                Text(part.failure ?? "").font(Theme.mono(11)).lineSpacing(5).foregroundStyle(palette.danger).textSelection(.enabled)
            }
        }
        .task(id: "\(part.tool.toolCallId)|\(part.tool.running)|\(part.output?.lines ?? -1)") {
            guard let read = context.toolDetail else {
                problem = "the host is not here to send it"
                return
            }
            do {
                detail = try await read(part.tool.toolCallId, !part.tool.running)
            } catch {
                problem = Sikemux.problem(error)
            }
        }
    }
}

private struct Terminal: View {
    @Environment(\.palette) private var palette
    @Environment(TranscriptContext.self) private var context
    let part: ToolPart
    let command: String?
    let group: String
    let output: ToolOutput?
    /// Stands in for the output until the host has sent it.
    let waiting: String?

    /// Lines of a call's output shown before "Show all".
    private static let foldLines = 12

    var body: some View {
        let text = output?.text ?? ""
        let lines = text.isEmpty ? [] : text.components(separatedBy: "\n")
        let folds = lines.count > Self.foldLines
        let whole = context.folds.isOpen("\(group)/\(part.id)/whole", false)
        let folded = folds && !whole
        VStack(alignment: .leading, spacing: 0) {
            if let command {
                HStack(alignment: .top, spacing: 7) {
                    Text("$").font(Theme.mono(11.5)).foregroundStyle(palette.toolRun).padding(.vertical, 4)
                    Text(command).font(Theme.mono(11.5)).lineSpacing(3).foregroundStyle(palette.ink).textSelection(.enabled)
                        .padding(.vertical, 4).frame(maxWidth: .infinity, alignment: .leading)
                    CopyButton(value: command, label: "command", size: 12)
                }
                .padding(.leading, 9)
                .padding(.trailing, 3)
                .padding(.vertical, 3)
            }
            if let image = output?.image, let picture = DataPicture(base64: image.data) {
                Button { context.showImage(picture.image, "attachment") } label: {
                    Image(uiImage: picture.image).resizable().scaledToFit().frame(maxWidth: .infinity)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Picture the call returned, open")
            }
            if !text.isEmpty {
                ZStack(alignment: .topTrailing) {
                    if folded {
                        ScrollView(.horizontal, showsIndicators: false) {
                            outputText(lines.prefix(Self.foldLines).joined(separator: "\n"))
                        }
                    } else {
                        ScrollView([.vertical, .horizontal], showsIndicators: false) {
                            outputText(text).textSelection(.enabled)
                        }
                        .frame(maxHeight: 320)
                    }
                    CopyButton(value: text, label: "output", size: 12).padding(3)
                }
                .overlay(alignment: .top) { if command != nil { Rectangle().fill(palette.border).frame(height: 1) } }
            } else if let waiting {
                note(waiting, ruled: command != nil)
            } else if part.output != nil, output?.image == nil {
                note("No output", ruled: command != nil)
            }
            if folds {
                Button { context.folds.set("\(group)/\(part.id)/whole", !whole) } label: {
                    Text(folded ? "Show all \(lines.count) lines" : "Show fewer lines")
                        .font(Theme.mono(10.5)).foregroundStyle(palette.inkFaint)
                        .padding(.horizontal, 9).padding(.vertical, 7)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .buttonStyle(.plain)
                .overlay(alignment: .top) { Rectangle().fill(palette.border).frame(height: 1) }
            }
            if (output?.cut ?? part.output?.cut) == true, !folds || whole {
                note("The rest of the output was not kept", ruled: true)
            }
            if part.tool.status == .failed, let code = part.output?.exitCode {
                Text("exit \(code)").font(Theme.mono(10.5)).foregroundStyle(palette.danger).padding(.horizontal, 9).padding(.bottom, 7)
            }
        }
        .background(palette.sunken)
        .clipShape(RoundedRectangle(cornerRadius: 8))
        .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(palette.border, lineWidth: 1))
    }

    private func outputText(_ text: String) -> some View {
        Text(text)
            .font(Theme.mono(11))
            .lineSpacing(16 - 11 * 1.32)
            .foregroundStyle(palette.inkDim)
            .fixedSize()
            .padding(.leading, 9)
            .padding(.trailing, 33)
            .padding(.vertical, 7)
    }

    private func note(_ text: String, ruled: Bool) -> some View {
        Text(text)
            .font(Theme.mono(10.5))
            .foregroundStyle(palette.inkFaint)
            .padding(.horizontal, 9)
            .padding(.vertical, 6)
            .frame(maxWidth: .infinity, alignment: .leading)
            .overlay(alignment: .top) { if ruled { Rectangle().fill(palette.border).frame(height: 1) } }
    }
}

/// A picture sent inline as base64.
struct DataPicture {
    let image: UIImage

    init?(base64: String) {
        guard let data = Data(base64Encoded: base64, options: .ignoreUnknownCharacters), let image = UIImage(data: data) else { return nil }
        self.image = image
    }
}
