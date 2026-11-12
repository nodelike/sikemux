import SwiftUI

/// A notice as a plain line: its icon, then its words, at the transcript's left with a little room around.
private struct NoticeLine<Content: View>: View {
    @ViewBuilder let content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 2) { content }
            .padding(.vertical, 4)
            .padding(.leading, 12)
            .padding(.vertical, 2)
            .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// A notice's words after its name, kept to one line.
private func inline(_ text: String, _ palette: Palette) -> some View {
    Text(text).font(Theme.ui(12.5)).foregroundStyle(palette.inkDim).lineLimit(1).layoutPriority(-1)
}

/// A background task that reached its end, and what it said of how it went.
struct TaskNoticeView: View {
    @Environment(\.palette) private var palette
    @Environment(TranscriptContext.self) private var context
    let id: String
    let notice: TaskNotice

    var body: some View {
        let summary = notice.summary?.trimmingCharacters(in: .whitespacesAndNewlines).nilIfEmpty
        let open = context.folds.isOpen("notice/\(id)", false)
        let failed = notice.state == .failed
        Button {
            context.folds.set("notice/\(id)", !open)
        } label: {
            NoticeLine {
                HStack(spacing: 8) {
                    IconView(icon: notice.taskType == "workflow" ? Icons.workflow : Icons.timer, size: 12)
                        .foregroundStyle(palette.inkDim)
                    Text(notice.name).font(Theme.mono(12)).foregroundStyle(palette.ink).lineLimit(1)
                    Text(notice.state.word).font(Theme.ui(12.5)).foregroundStyle(
                        failed ? palette.danger : palette.inkDim
                    )
                    .fixedSize()
                    if let summary, !open { inline(summary, palette) }
                }
                .frame(minHeight: 18)
                if let summary, open {
                    Text(summary).font(Theme.ui(12.5)).lineSpacing(18 - 12.5 * 1.32).foregroundStyle(palette.inkDim)
                        .multilineTextAlignment(.leading)
                        .padding(.leading, 20)
                        .textSelection(.enabled)
                }
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(summary == nil)
        .accessibilityLabel("\(notice.name) \(notice.state.word)\(summary.map { ". \($0)" } ?? "")")
        .accessibilityValue(summary == nil ? "" : open ? "expanded" : "collapsed")
    }
}

extension NoticeState {
    fileprivate var word: String {
        switch self {
        case .completed: String(localized: "completed")
        case .failed: String(localized: "failed")
        case .stopped: String(localized: "stopped")
        case .unknown: rawValue
        }
    }
}

/// A task notice that names one of the chat's subagents draws as that subagent's row.
struct TaskNoticeOrCard: View {
    @Environment(TranscriptContext.self) private var context
    let id: String
    let notice: TaskNotice

    var body: some View {
        if let session = notice.subagent, let card = context.subagent(session) {
            SubagentRow(subagent: card)
        } else {
            TaskNoticeView(id: id, notice: notice)
        }
    }
}

struct AgentNoticeLine: View {
    @Environment(\.palette) private var palette
    let notice: AgentNotice

    var body: some View {
        if let goal = notice.goal {
            GoalLine(notice: notice, goal: goal)
        } else if let overruled = notice.overruled {
            OverruledLine(overruled: overruled)
        } else if let week = notice.week {
            WeekLine(week: week)
        } else {
            let tint =
                notice.severity == .error ? palette.danger : notice.severity == .warning ? palette.warn : palette.inkDim
            NoticeLine {
                HStack(spacing: 8) {
                    IconView(icon: notice.severity == .info ? Icons.inbox : Icons.warning, size: 12)
                        .foregroundStyle(tint)
                    Text(notice.title).font(Theme.ui(12.5)).foregroundStyle(
                        notice.severity == .error ? tint : palette.inkDim
                    )
                    .fixedSize()
                    if let description = notice.description { inline(description, palette) }
                }
                .frame(minHeight: 18)
            }
            .accessibilityElement(children: .ignore)
            .accessibilityLabel("\(notice.title)\(notice.description.map { ". \($0)" } ?? "")")
        }
    }
}

struct AccountLine: View {
    @Environment(\.palette) private var palette
    let move: AccountMove

    var body: some View {
        let why = move.reason == .limit ? move.from.map { String(localized: "\($0) reached its usage limit") } : nil
        NoticeLine {
            HStack(spacing: 8) {
                IconView(icon: Icons.user, size: 12).foregroundStyle(palette.inkDim)
                Text("Moved to").font(Theme.ui(12.5)).foregroundStyle(palette.inkDim).fixedSize()
                Text(move.label).font(Theme.ui(12.5)).foregroundStyle(palette.ink).lineLimit(1)
                if let why { inline(why, palette) }
            }
            .frame(minHeight: 18)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(String(localized: "Moved to \(move.label)") + (why.map { ". \($0)" } ?? ""))
    }
}

/// The account smart accounts picked for a chat about to start, and why, in one quiet centred line.
struct SmartPick: View {
    @Environment(\.palette) private var palette
    let move: AccountMove
    let provider: String

    var body: some View {
        VStack(spacing: 4) {
            HStack(spacing: 7) {
                AgentMark(provider: provider, size: 12)
                Text(move.label).font(Theme.ui(12.5, .medium)).foregroundStyle(palette.ink).lineLimit(1)
                Text("SMART PICK").font(Theme.ui(10, .medium)).tracking(1.6).foregroundStyle(palette.tertiary)
            }
            if let placed = move.placed {
                Text(placed).font(Theme.ui(12)).foregroundStyle(palette.inkFaint).multilineTextAlignment(.center)
            }
        }
        .frame(maxWidth: .infinity)
        .padding(.top, 40)
        .padding(.horizontal, 16)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(String(localized: "Smart pick: \(move.label)") + (move.placed.map { ". \($0)" } ?? ""))
    }
}

/// A file or picture in a row, named when it cannot be shown.
struct ResourceChip: View {
    @Environment(\.palette) private var palette
    let name: String
    let image: Bool
    var trailing: String?

    var body: some View {
        HStack(spacing: 8) {
            IconView(icon: image ? Icons.image : Icons.file, size: 14).foregroundStyle(palette.inkDim)
            Text(name).font(Theme.mono(12)).foregroundStyle(palette.ink).lineLimit(1).textSelection(.enabled)
            if let trailing { Text(trailing).font(Theme.ui(12)).foregroundStyle(palette.inkFaint) }
        }
        .padding(.vertical, 7)
        .padding(.horizontal, 10)
        .background(RoundedRectangle(cornerRadius: 8).fill(palette.raised))
        .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(palette.border, lineWidth: 1))
        .padding(.vertical, 6)
        .accessibilityLabel("File \(name)")
    }
}

/// A page the agent showed. The phone has no web view to draw it in, so it names the page and where it is.
struct PageCard: View {
    @Environment(\.palette) private var palette
    let page: PageRef

    var body: some View {
        HStack(spacing: 8) {
            IconView(icon: Icons.window, size: 14).foregroundStyle(palette.inkDim)
            Text(page.title).font(Theme.ui(13, .medium)).foregroundStyle(palette.ink).lineLimit(1)
            Text("on the computer").font(Theme.ui(12)).foregroundStyle(palette.inkFaint)
        }
        .padding(.vertical, 7)
        .padding(.horizontal, 10)
        .background(RoundedRectangle(cornerRadius: 8).fill(palette.raised))
        .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(palette.border, lineWidth: 1))
        .padding(.vertical, 6)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Page \(page.title), open on the computer")
    }
}

/// A picture or file the agent put in its reply.
struct ContentView: View {
    @Environment(\.palette) private var palette
    @Environment(TranscriptContext.self) private var context
    let block: ContentBlock

    var body: some View {
        switch block {
        case .text(let text) where !text.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty:
            MarkdownView(blocks: plainBlocks(text.text), style: .prose(palette))
        case .image(let image) where image.data != nil && image.mimeType.hasPrefix("image/"):
            if let picture = DataPicture(base64: image.data!) {
                InlinePicture(image: picture.image, name: attachmentName(image.mimeType))
            }
        default:
            if let name {
                if let path = uri.flatMap(HostFiles.imagePath) {
                    HostPicture(path: path, name: name)
                } else {
                    ResourceChip(name: name, image: mimeType?.hasPrefix("image/") == true)
                }
            }
        }
    }

    private var uri: String? {
        switch block {
        case .image(let image): image.uri
        case .resourceLink(let link): link.uri
        case .resource(let resource): resource.resource.uri
        default: nil
        }
    }

    private var mimeType: String? {
        switch block {
        case .image(let image): image.mimeType
        case .audio(let audio): audio.mimeType
        case .resourceLink(let link): link.mimeType
        default: nil
        }
    }

    private var name: String? {
        if case .resourceLink(let link) = block { return link.title ?? link.name }
        return uri.map { ($0 as NSString).lastPathComponent }
    }
}

/// An attachment arrives as bytes and a type, with nothing naming it, so the type is the only thing that can say what
/// it would be saved as.
func attachmentName(_ mimeType: String) -> String {
    let kind =
        (mimeType.split(separator: "/").last ?? "").split(separator: "+").first.map {
            String($0).filter { $0.isLetter || $0.isNumber }
        } ?? ""
    return "attachment.\(kind.isEmpty ? "png" : kind)"
}

/// A picture in the transcript, opened full screen on a tap.
struct InlinePicture: View {
    @Environment(\.palette) private var palette
    @Environment(TranscriptContext.self) private var context
    let image: UIImage
    let name: String

    var body: some View {
        Button {
            context.showImage(image, name)
        } label: {
            Image(uiImage: image)
                .resizable()
                .scaledToFit()
                .frame(maxWidth: .infinity)
                .aspectRatio(4 / 3, contentMode: .fit)
                .background(palette.sunken)
                .clipShape(RoundedRectangle(cornerRadius: 8))
        }
        .buttonStyle(.plain)
        .padding(.vertical, 6)
        .accessibilityLabel("\(name), open")
    }
}

/// A picture the answer links to on the host, previewed as the Mac previews it, or its file until it arrives.
struct HostPicture: View {
    @Environment(TranscriptContext.self) private var context
    let path: String
    let name: String
    @State private var image: UIImage?

    var body: some View {
        Group {
            if let image { InlinePicture(image: image, name: name) } else { ResourceChip(name: name, image: true) }
        }
        .task(id: path) {
            guard let core = context.core, let file = await HostFiles.shared.read(core: core, path: path) else {
                return
            }
            image = UIImage(contentsOfFile: file.path)
        }
    }
}

private let subagentWords: [SubagentState: String] = [
    .running: String(localized: "working"), .completed: String(localized: "done"), .failed: String(localized: "failed"),
    .cancelled: String(localized: "stopped"), .disconnected: String(localized: "lost"),
]

/// A subagent as a strip: who, its task, what it is doing now or how it ended, and what it cost; it opens onto its work.
struct SubagentRow: View {
    @Environment(\.palette) private var palette
    @Environment(TranscriptContext.self) private var context
    let subagent: SubagentCard

    var body: some View {
        let open = context.folds.isOpen("subagent/\(subagent.sessionId)", false)
        let cost = Self.cost(subagent)
        let running = subagent.state == .running
        let row = subagent.lastTool?.row
        let doing = running ? row.map { $0.activity ?? $0.subject ?? $0.target } : nil
        VStack(alignment: .leading, spacing: 0) {
            Button {
                context.folds.set("subagent/\(subagent.sessionId)", !open)
            } label: {
                HStack(spacing: 8) {
                    AgentMark(provider: context.provider, size: 16)
                    Text(subagent.name).font(Theme.ui(12.5, .semibold)).foregroundStyle(palette.ink).lineLimit(1)
                        .frame(maxWidth: 120, alignment: .leading).fixedSize()
                    Text(subagent.headline).font(Theme.ui(12.5)).foregroundStyle(palette.inkDim).lineLimit(1)
                        .layoutPriority(-1)
                    if let doing {
                        Shimmer(text: doing, font: Theme.ui(12.5), color: palette.secondary).layoutPriority(-1)
                    } else {
                        SubagentEnd(state: subagent.state).fixedSize()
                    }
                    Spacer(minLength: 0)
                    if let cost {
                        Text(cost).font(Theme.ui(12)).monospacedDigit().foregroundStyle(palette.inkFaint).lineLimit(1)
                            .fixedSize()
                    }
                    IconView(icon: Icons.chevron, size: 11).foregroundStyle(palette.inkDim)
                        .rotationEffect(.degrees(open ? 90 : 0))
                }
                .padding(.horizontal, 12)
                .frame(height: 34)
                .contentShape(Rectangle())
            }
            .buttonStyle(ActiveFill(radius: 10))
            .accessibilityLabel(
                String(localized: "Subagent \(subagent.name), \(doing ?? subagentWords[subagent.state] ?? "")")
                    + (cost.map { ", \($0)" } ?? "")
            )
            .accessibilityValue(open ? "expanded" : "collapsed")
            if open {
                SubagentWork(subagent: subagent, running: running)
                    .padding(.leading, 25)
                    .padding(.trailing, 10)
                    .padding(.top, 2)
                    .padding(.bottom, 8)
            }
        }
        .background(palette.raised.opacity(0.55), in: RoundedRectangle(cornerRadius: 10))
        .padding(.vertical, 4)
    }

    static func cost(_ subagent: SubagentCard) -> String? {
        var pieces: [String] = []
        if let tokens = subagent.tokens, tokens > 0 {
            pieces.append(String(localized: "\(Format.tokens(tokens)) tokens"))
        }
        if subagent.calls > 0 { pieces.append(ChatWords.callCount(subagent.calls)) }
        return pieces.isEmpty ? nil : pieces.joined(separator: " · ")
    }
}

/// What an opened subagent strip holds: the subagent's own rows, read only while it is open.
private struct SubagentWork: View {
    @Environment(\.palette) private var palette
    @Environment(TranscriptContext.self) private var context
    let subagent: SubagentCard
    let running: Bool

    var body: some View {
        let parts = (context.subagentRows[subagent.sessionId] ?? []).flatMap(\.parts)
        if parts.isEmpty {
            Text("No output yet.").font(Theme.ui(12.5)).foregroundStyle(palette.inkFaint).padding(.vertical, 6)
        } else {
            Parts(id: "subagent/\(subagent.sessionId)", parts: parts, live: running)
        }
    }
}

/// A subagent's mark and word once it is not doing anything: done, failed, lost or stopped.
private struct SubagentEnd: View {
    @Environment(\.palette) private var palette
    let state: SubagentState

    var body: some View {
        let loud = state == .failed || state == .disconnected
        let tint =
            switch state {
            case .completed: palette.live.mixed(with: palette.inkFaint, 0.58)
            case .failed, .disconnected: palette.danger
            case .running: palette.accent.mixed(with: palette.inkFaint, 0.62)
            default: palette.inkFaint
            }
        let icon =
            switch state {
            case .running: Icons.hourglass
            case .completed: Icons.check
            case .failed: Icons.warning
            case .disconnected: Icons.plug
            default: Icons.close
            }
        HStack(spacing: 6) {
            IconView(icon: icon, size: 12).foregroundStyle(tint)
            Text(subagentWords[state] ?? "").font(Theme.ui(12.5)).foregroundStyle(loud ? tint : palette.inkDim)
        }
    }
}

/// Where a chat with a worktree of its own runs, above its first message: its branch, then its folder more quietly.
struct WorktreeLine: View {
    @Environment(\.palette) private var palette
    let worktree: AgentWorktree

    var body: some View {
        HStack(spacing: 10) {
            IconView(icon: Icons.git, size: 14)
                .foregroundStyle(palette.accent)
                .frame(width: 30, height: 30)
                .background(RoundedRectangle(cornerRadius: 7).fill(palette.accent.opacity(0.16)))
            VStack(alignment: .leading, spacing: 2) {
                Text(
                    "Working in a worktree on \(Text(worktree.branch).font(Theme.mono(12)).foregroundStyle(palette.ink))"
                )
                .font(Theme.ui(12)).foregroundStyle(palette.tertiary).lineLimit(1)
                Text(worktree.path).font(Theme.mono(11.5)).foregroundStyle(palette.inkFaint).lineLimit(1)
                    .truncationMode(.head)
            }
        }
        .padding(.top, 12)
        .padding(.bottom, 4)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Working in a worktree on \(worktree.branch), at \(worktree.path)")
    }
}

/// A trigger's prompt in a bubble at the left under the trigger's name, with the webhook request that fired it folded
/// beneath.
struct FiredTrigger: View {
    @Environment(\.palette) private var palette
    @Environment(\.displayScale) private var scale
    @Environment(TranscriptContext.self) private var context
    let id: String
    let fired: TriggerFired
    var at: Int?

    var body: some View {
        let open = context.folds.isOpen("\(id)/request", false)
        VStack(alignment: .leading, spacing: 0) {
            From(at: at) {
                IconView(icon: Icons.timer, size: 13).foregroundStyle(palette.cmd)
                Text(fired.title).font(Theme.ui(12.5, .semibold)).foregroundStyle(palette.ink).lineLimit(1)
            } content: {
                Text(fired.prompt).sentWords(palette).textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if let request = fired.request {
                Button {
                    context.folds.set("\(id)/request", !open)
                } label: {
                    HStack(spacing: 6) {
                        IconView(icon: Icons.chevron, size: 10).foregroundStyle(palette.inkFaint).rotationEffect(
                            .degrees(open ? 90 : 0))
                        Text(Self.summary(request)).font(Theme.mono(11)).foregroundStyle(palette.inkFaint).lineLimit(1)
                    }
                    .padding(.vertical, 4).padding(.horizontal, 8)
                }
                .buttonStyle(.plain)
                .padding(.top, 6)
                .accessibilityLabel("The request that fired it: \(Self.summary(request))")
                if open {
                    VStack(alignment: .leading, spacing: 8) {
                        VStack(alignment: .leading, spacing: 0) {
                            ForEach(request.headers, id: \.name) { header in
                                Text("\(Text(header.name).foregroundStyle(palette.inkFaint)) \(header.value)")
                                    .font(Theme.mono(11)).foregroundStyle(palette.ink).textSelection(.enabled)
                            }
                        }
                        if let tree = request.bodyTree {
                            LongText(
                                id: "\(id)/request/body", text: request.body, md: tree, live: false,
                                style: .prose(palette))
                        }
                    }
                    .padding(10)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(RoundedRectangle(cornerRadius: 8).fill(palette.raised))
                    .overlay(
                        RoundedRectangle(cornerRadius: 8).strokeBorder(
                            palette.border, lineWidth: 1 / scale)
                    )
                    .padding(.top, 4)
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    static func summary(_ request: FiredRequest) -> String {
        let kind = request.headers.first { $0.name == "content-type" }?.value.split(separator: ";").first?
            .trimmingCharacters(in: .whitespaces)
        let bytes = request.body.utf16.count
        let size = Format.bytes(bytes)
        return [request.method, kind, size, request.query.isEmpty ? nil : "?\(request.query)"].compactMap { $0 }.filter
        { !$0.isEmpty }.joined(separator: " · ")
    }
}

/// The line under the last row: the agent's mark breathing, a sweep on what it is doing, then what it is on and for
/// how long.
struct ActivityLine: View {
    @Environment(\.palette) private var palette
    @Environment(\.accessibilityReduceMotion) private var still
    let provider: String
    let doing: Doing
    let since: Date
    @State private var breath = false

    var body: some View {
        HStack(spacing: 8) {
            AgentMark(provider: provider, size: 20)
                .opacity(breath ? 0.5 : 1)
                .onAppear {
                    guard !still else { return }
                    withAnimation(.easeInOut(duration: 0.75).repeatForever()) { breath = true }
                }
            Shimmer(text: doing.verb, font: Theme.ui(13, .medium), color: palette.inkDim).fixedSize()
            if let what = doing.what {
                Text(what).font(doing.typed ? Theme.mono(12) : Theme.ui(12.5)).foregroundStyle(palette.secondary)
                    .lineLimit(1)
                    .layoutPriority(-1)
            }
            TimelineView(.periodic(from: since, by: 1)) { timeline in
                let seconds = Int(timeline.date.timeIntervalSince(since).rounded())
                if seconds > 0 {
                    Text(ChatWords.duration(ms: Double(seconds) * 1000)).font(Theme.mono(10.5)).foregroundStyle(
                        palette.inkFaint
                    )
                    .fixedSize()
                }
            }
        }
        .padding(.top, 14)
        .padding(.bottom, 6)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(doing.what.map { "\(doing.verb) \($0)" } ?? doing.verb)
    }
}

/// Above the first message while the host has turns from before it, which arrive as it comes into view.
struct EarlierLine: View {
    @Environment(\.palette) private var palette
    let failed: Bool
    let loading: Bool
    let onRetry: () -> Void

    var body: some View {
        Group {
            if failed {
                Button(action: onRetry) {
                    Text(
                        "Couldn't load earlier messages · \(Text("Retry").font(Theme.ui(12.5, .medium)).foregroundStyle(palette.ink))"
                    )
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Couldn't load earlier messages. Retry")
            } else {
                HStack(spacing: 8) {
                    if loading { Working() } else { Color.clear.frame(width: 13, height: 13) }
                    Text("Loading earlier messages…")
                }
            }
        }
        .font(Theme.ui(12.5))
        .foregroundStyle(palette.tertiary)
        .frame(maxWidth: .infinity)
        .padding(.top, 10)
        .padding(.bottom, 6)
    }
}

/// Messages waiting behind the running turn, faded where they will go.
struct QueuedLines: View {
    @Environment(\.palette) private var palette
    let held: [QueuedMessage]

    var body: some View {
        VStack(alignment: .trailing, spacing: 0) {
            ForEach(held, id: \.id) { message in
                VStack(alignment: .trailing, spacing: 0) {
                    if !message.paths.isEmpty { SentAttachments(paths: message.paths) }
                    if !message.text.isEmpty {
                        Text(message.text)
                            .sentWords(palette)
                            .mineBubble(palette)
                            .sentWidth()
                    }
                }
                .opacity(0.55)
                .padding(.bottom, 4)
            }
            Text("Sends when this turn ends").font(Theme.ui(11)).foregroundStyle(palette.inkFaint).padding(.top, 4)
        }
        .frame(maxWidth: .infinity, alignment: .trailing)
        .padding(.top, 20)
        .padding(.bottom, 10)
    }
}

extension String {
    var nilIfEmpty: String? { isEmpty ? nil : self }
}
