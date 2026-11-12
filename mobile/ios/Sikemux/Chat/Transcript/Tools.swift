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
    "activity": "IconActivity", "agent": "IconAgent", "agents": "IconAgents", "aws": "IconAws",
    "bitbucket": "IconBitbucket", "board": "IconBoard", "bolt": "IconBolt", "book": "IconBook",
    "camera": "IconCamera", "check": "IconCheck", "click": "IconClick", "clock": "IconClock", "close": "IconClose",
    "code": "IconCode", "command": "IconCommand", "commit": "IconCommit", "cube": "IconCube",
    "dashboard": "IconDashboard", "download": "IconDownload", "eye": "IconEye", "file": "IconFile",
    "file-plus": "IconFilePlus", "flask": "IconFlask", "folder": "IconFolder",
    "forgejo": "IconForgejo", "git": "IconGit", "gitea": "IconGitea", "github": "IconGithub",
    "gitlab": "IconGitlab", "globe": "IconGlobe", "highlighter": "IconHighlighter",
    "home-button": "IconHomeButton", "hourglass": "IconHourglass", "image": "IconImage", "issue": "IconIssue",
    "jira": "IconJira", "keyboard": "IconKeyboard", "linear": "IconLinear", "link": "IconLink",
    "log-lines": "IconLogLines", "logo": "Logo", "message": "IconMessage", "mouse": "IconMouse", "move": "IconMove",
    "network": "IconNetwork", "panel": "IconPanel", "pencil": "IconPencil", "phone": "IconPhone",
    "pinch": "IconPinch", "plug": "IconPlug", "pointer": "IconPointer", "pull-request": "IconPullRequest",
    "push": "IconPush", "record": "IconRecord", "refresh": "IconRefresh", "resize": "IconResize",
    "rocket": "IconRocket", "rotate": "IconRotate", "run": "IconRun", "rundeck": "IconRundeck",
    "search": "IconSearch", "server": "IconServer", "share": "IconShare", "slack": "IconSlack",
    "square-plus": "IconSquarePlus", "status": "IconStatus", "steps": "IconSteps", "stop": "IconStop",
    "swipe": "IconSwipe", "tabs": "IconTabs", "tap": "IconTap", "terminal": "IconTerminal", "text": "IconText",
    "timer": "IconTimer", "touch-path": "IconTouchPath", "trash": "IconTrash", "unlink": "IconUnlink",
    "upload": "IconUpload", "user": "IconUser", "vercel": "IconVercel", "warning": "IconWarning",
    "waterfall": "IconWaterfall", "window": "IconWindow", "workflow": "IconWorkflow", "wrench": "IconWrench",
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
    let color: Color =
        switch tool.row.action {
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
            Text(
                ChatWords.elapsed(
                    seconds: max(0, Int((spent / 1000 + context.date.timeIntervalSince(since)).rounded(.down))))
            )
            .font(font)
            .foregroundStyle(color)
            .monospacedDigit()
        }
    }
}

extension ToolPart {
    var startDate: Date? { startedAt.map { Format.date(millis: $0) } }

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
        let total = parts.reduce(0) { $0 + ($1.tool.row.times ?? 1) }
        let calls = ChatWords.calls(total)
        VStack(alignment: .leading, spacing: 0) {
            Button {
                context.folds.set(id, !open)
            } label: {
                HStack(spacing: 8) {
                    if current != nil {
                        Shimmer(
                            text: said ?? calls, font: Theme.ui(12.5),
                            color: said != nil ? palette.secondary : palette.inkDim)
                    } else {
                        Text(calls).font(Theme.ui(12.5)).foregroundStyle(palette.inkDim).lineLimit(1)
                    }
                    if said != nil, total > 1 {
                        Text(ChatWords.callCount(total)).font(Theme.mono(11)).foregroundStyle(palette.inkDim)
                    }
                    if let current {
                        LiveSeconds(
                            since: current.startDate ?? Date(), spent: spent, font: Theme.mono(11),
                            color: palette.inkDim
                        )
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
    @Environment(\.displayScale) private var scale
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
        let opens =
            part.diff != nil || part.output != nil || part.failure != nil
            || (command != nil && (running || command != tool.row.target))
        let open = context.folds.isOpen("\(group)/\(part.id)", false)
        VStack(alignment: .leading, spacing: 0) {
            Button {
                if opens { context.folds.set("\(group)/\(part.id)", !open) }
            } label: {
                HStack(spacing: 8) {
                    Rectangle().fill(palette.treeTick).frame(width: 10, height: 1).opacity(last && !open ? 0 : 1)
                    IconView(icon: icon, size: 12).foregroundStyle(color)
                    Text(ChatWords.toolKind(tool)).font(Theme.mono(11.5)).foregroundStyle(color).lineLimit(1)
                        .frame(minWidth: kindWidth, alignment: .leading)
                    if running {
                        Shimmer(
                            text: tool.row.target + (tool.row.detail.map { " \($0)" } ?? ""), font: Theme.mono(11.5),
                            color: palette.inkDim
                        )
                        .frame(maxWidth: .infinity, alignment: .leading)
                    } else {
                        target(tool)
                            .font(Theme.mono(11.5))
                            .lineLimit(1)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    HStack(spacing: 6) {
                        if let diff = part.diff {
                            DiffText.counts(adds: diff.adds, dels: diff.dels, palette)
                                .font(Theme.mono(10.5))
                        }
                        if let times = part.tool.row.times {
                            Text("×\(times)").font(Theme.mono(10.5)).foregroundStyle(palette.inkDim)
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
            .allowsHitTesting(opens)
            .accessibilityLabel(
                "\(ChatWords.toolKind(tool)) \(tool.row.target)\(tool.status == .failed ? ", failed" : running ? ", running" : "")"
            )
            if open {
                ToolDetailView(part: part, command: command, group: group)
                    .padding(.leading, 18)
                    .padding(.top, 2)
                    .padding(.bottom, 6)
                    .background(alignment: .leading) {
                        if !last { Rectangle().fill(palette.treeSpine).frame(width: 1) }
                    }
            }
        }
        .opacity(tool.status == .cancelled ? 0.6 : 1)
    }

    private func target(_ tool: ToolCall) -> Text {
        let target = tool.row.target
        var words = AttributedString(target)
        words.foregroundColor = palette.ink
        if let path = tool.row.path {
            words = FileGlyph.mark(size: 11) + AttributedString(" ") + words
            words.foregroundColor = palette.ink
            words.link = TranscriptLink.file(path)
        } else if let range = target.range(of: #"https?://[^\s<>"'`]+"#, options: .regularExpression) {
            var raw = String(target[range])
            while let last = raw.last, ".,;:!?)]".contains(last) { raw.removeLast() }
            if let url = TranscriptLink.openable(raw), let found = words.range(of: raw) {
                words[found].link = url
                words[found].foregroundColor = palette.accent
            }
        }
        var text = FileGlyph.text(words, ink: palette.inkDim, scale: scale)
        if let detail = tool.row.detail { text = Text("\(text)\(Text(" \(detail)").foregroundStyle(palette.inkDim))") }
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
                    path.addQuadCurve(
                        to: CGPoint(x: 4.5, y: size.size.height / 2), control: CGPoint(x: 0.5, y: size.size.height / 2))
                    path.addLine(to: CGPoint(x: 10, y: size.size.height / 2))
                }
                .stroke(palette.treeSpine, lineWidth: 1)
            } else {
                Rectangle().fill(palette.treeSpine).frame(width: 1)
            }
        }
    }
}

/// What an opened call did: its change, what it printed, or why it failed. It is read from the host when the row opens
/// and again as a running call goes on.
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
            if part.diff != nil {
                DiffBody(
                    lines: detail?.diff?.lines ?? [], fold: "\(group)/\(part.id)/diff",
                    waiting: detail?.diff != nil ? nil : waiting, folds: context.folds)
            } else if command != nil || part.output != nil {
                Terminal(
                    part: part, command: command.flatMap { $0 != part.tool.row.target ? $0 : nil }, group: group,
                    output: detail?.output, waiting: part.output != nil ? waiting : nil)
            } else {
                Text(part.failure ?? "").font(Theme.mono(11)).lineSpacing(17 - 11 * 1.32).foregroundStyle(
                    palette.danger
                )
                .textSelection(.enabled)
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

/// What a command printed: its exit code and size, then its last lines, the rest a tap away.
private struct Terminal: View {
    @Environment(\.palette) private var palette
    @Environment(TranscriptContext.self) private var context
    let part: ToolPart
    /// The whole command, when its line had to cut it short.
    let command: String?
    let group: String
    let output: ToolOutput?
    /// Stands in for the output until the host has sent it.
    let waiting: String?

    /// Lines of what a command printed shown before "Show all": its last ones, as on the Mac.
    private static let tailLines = 8

    var body: some View {
        let text = Self.trimmed(output?.text ?? "")
        let lines = text.isEmpty ? [] : text.components(separatedBy: "\n")
        let folds = lines.count > Self.tailLines
        let whole = context.folds.isOpen("\(group)/\(part.id)/whole", false)
        let shown = folds && !whole ? lines.suffix(Self.tailLines).joined(separator: "\n") : text
        let exit =
            part.tool.status == .failed ? (part.output?.exitCode.map { "exit \($0)" } ?? "failed") : nil
        VStack(alignment: .leading, spacing: 0) {
            if let command {
                HStack(alignment: .top, spacing: 7) {
                    Text("$").font(Theme.mono(11.5)).foregroundStyle(palette.toolRun).padding(.vertical, 4)
                    Text(command).font(Theme.mono(11.5)).lineSpacing(17 - 11.5 * 1.32).foregroundStyle(palette.ink)
                        .textSelection(.enabled)
                        .padding(.vertical, 4).frame(maxWidth: .infinity, alignment: .leading)
                    CopyButton(value: command, label: "command", size: 12)
                }
                .padding(.leading, 12)
                .padding(.trailing, 3)
                .padding(.vertical, 3)
                .background(palette.sunken, in: RoundedRectangle(cornerRadius: 8))
                .padding(.bottom, 4)
            }
            if exit != nil || !lines.isEmpty {
                HStack(spacing: 10) {
                    if let exit {
                        Text(exit).font(Theme.mono(11)).foregroundStyle(palette.danger.mixed(with: palette.ink, 0.7))
                            .padding(.horizontal, 7).padding(.vertical, 1)
                            .background(palette.danger.opacity(0.12), in: Capsule())
                    }
                    if !lines.isEmpty {
                        Text("\(lines.count) \(Format.counted("line", lines.count))").font(Theme.ui(12))
                            .foregroundStyle(palette.inkFaint)
                    }
                    Spacer(minLength: 0)
                    if folds {
                        Button {
                            context.folds.set("\(group)/\(part.id)/whole", !whole)
                        } label: {
                            HStack(spacing: 5) {
                                IconView(icon: Icons.chevron, size: 9).foregroundStyle(palette.inkDim)
                                    .rotationEffect(.degrees(whole ? 90 : 0))
                                Text(whole ? "Show less" : "Show all").font(Theme.ui(12)).foregroundStyle(
                                    palette.inkDim)
                            }
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                    }
                }
                .frame(height: 26)
            }
            if let image = output?.image, let picture = DataPicture(base64: image.data) {
                Button {
                    context.showImage(picture.image, "attachment")
                } label: {
                    Image(uiImage: picture.image).resizable().scaledToFit().frame(maxWidth: .infinity)
                        .background(palette.sunken)
                        .clipShape(RoundedRectangle(cornerRadius: 8))
                }
                .buttonStyle(.plain)
                .padding(.top, 4)
                .accessibilityLabel("Picture the call returned, open")
            }
            if !shown.isEmpty {
                ZStack(alignment: .topTrailing) {
                    if whole {
                        ScrollView([.vertical, .horizontal], showsIndicators: false) {
                            printed(shown).textSelection(.enabled)
                        }
                        .frame(maxHeight: 320)
                    } else {
                        ScrollView(.horizontal, showsIndicators: false) {
                            printed(shown)
                        }
                    }
                    CopyButton(value: text, label: "output", size: 12).padding(4)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(palette.sunken)
                .clipShape(RoundedRectangle(cornerRadius: 8))
            } else if let waiting {
                note(waiting)
            } else if part.output != nil, output?.image == nil {
                note("No output")
            }
            if (output?.cut ?? part.output?.cut) == true, !folds || whole {
                note("The rest of the output was not kept")
            }
        }
    }

    private static func trimmed(_ text: String) -> String {
        var text = Substring(text)
        while text.last == "\n" { text.removeLast() }
        return String(text)
    }

    private func printed(_ text: String) -> some View {
        Text(text)
            .font(Theme.mono(11.5))
            .lineSpacing(19.5 - 11.5 * 1.32)
            .foregroundStyle(palette.secondary)
            .fixedSize()
            .padding(.leading, 12)
            .padding(.trailing, 30)
            .padding(.vertical, 10)
    }

    private func note(_ text: String) -> some View {
        Text(text)
            .font(Theme.ui(10.5))
            .foregroundStyle(palette.inkFaint)
            .padding(.top, 4)
            .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// A picture sent inline as base64.
struct DataPicture {
    let image: UIImage

    init?(base64: String) {
        guard let data = Data(base64Encoded: base64, options: .ignoreUnknownCharacters), let image = UIImage(data: data)
        else { return nil }
        self.image = image
    }
}
