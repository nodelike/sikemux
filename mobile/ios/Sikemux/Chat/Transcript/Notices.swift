import SwiftUI

private struct NoticeBox<Content: View>: View {
    @Environment(\.palette) private var palette
    @ViewBuilder let content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 3) { content }
            .padding(.vertical, 6)
            .padding(.horizontal, 10)
            .overlay(RoundedRectangle(cornerRadius: 9).strokeBorder(palette.border, lineWidth: 1))
            .padding(.vertical, 5)
    }
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
            NoticeBox {
                HStack(spacing: 7) {
                    IconView(icon: notice.taskType == "workflow" ? Icons.workflow : Icons.timer, size: 12)
                        .foregroundStyle(failed ? palette.danger : palette.inkFaint)
                    Text(notice.name).font(Theme.mono(11.5)).foregroundStyle(palette.ink).lineLimit(1)
                    Text(notice.state.rawValue).font(Theme.ui(12)).foregroundStyle(
                        failed ? palette.danger : palette.inkFaint)
                }
                if let summary {
                    Text(summary).font(Theme.ui(12)).lineSpacing(3).foregroundStyle(palette.inkFaint)
                        .lineLimit(open ? nil : 1)
                        .multilineTextAlignment(.leading)
                }
            }
        }
        .buttonStyle(.plain)
        .disabled(summary == nil)
        .accessibilityLabel("\(notice.name) \(notice.state.rawValue)\(summary.map { ". \($0)" } ?? "")")
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
        let tint =
            notice.severity == .error ? palette.danger : notice.severity == .warning ? palette.warn : palette.inkFaint
        NoticeBox {
            HStack(spacing: 7) {
                IconView(icon: notice.severity == .info ? Icons.info : Icons.warning, size: 12).foregroundStyle(tint)
                Text(notice.title).font(Theme.ui(12)).foregroundStyle(
                    notice.severity == .info ? palette.inkFaint : tint)
            }
            if let description = notice.description {
                Text(description).font(Theme.ui(12)).lineSpacing(3).foregroundStyle(palette.inkFaint).textSelection(
                    .enabled)
            }
        }
        .accessibilityElement(children: .combine)
    }
}

struct AccountLine: View {
    @Environment(\.palette) private var palette
    let move: AccountMove

    var body: some View {
        let why = move.reason == .limit && move.from != nil ? "\(move.from!) reached its usage limit" : move.placed
        NoticeBox {
            HStack(spacing: 7) {
                IconView(icon: Icons.user, size: 12).foregroundStyle(palette.inkFaint)
                Text(move.placed != nil ? "Started on" : "Moved to").font(Theme.ui(12)).foregroundStyle(
                    palette.inkFaint)
                Text(move.label).font(Theme.mono(11.5)).foregroundStyle(palette.ink).lineLimit(1)
            }
            if let why { Text(why).font(Theme.ui(12)).lineSpacing(3).foregroundStyle(palette.inkFaint) }
        }
        .accessibilityElement(children: .combine)
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
    .running: "working", .completed: "done", .failed: "failed", .cancelled: "stopped", .disconnected: "lost",
]

/// What a subagent did, kept to one line until it is opened; what it is doing now is over the composer.
struct SubagentRow: View {
    @Environment(\.palette) private var palette
    @Environment(TranscriptContext.self) private var context
    let subagent: SubagentCard

    var body: some View {
        let open = context.folds.isOpen("subagent/\(subagent.sessionId)", false)
        let rows = context.subagentRows[subagent.sessionId] ?? []
        let parts = rows.flatMap(\.parts)
        let cost = Self.cost(subagent)
        let running = subagent.state == .running
        let task = subagent.headline
        VStack(alignment: .leading, spacing: 0) {
            Button {
                context.folds.set("subagent/\(subagent.sessionId)", !open)
            } label: {
                HStack(spacing: 8) {
                    IconView(icon: Icons.chevron, size: 10).foregroundStyle(palette.inkDim).opacity(0.7).rotationEffect(
                        .degrees(open ? 90 : 0))
                    AgentMark(provider: context.provider, size: 15)
                    Text(subagent.name).font(Theme.mono(11.5)).foregroundStyle(palette.ink).lineLimit(1)
                        .frame(maxWidth: 140, alignment: .leading).fixedSize()
                    Group {
                        if running {
                            Shimmer(text: task, font: Theme.ui(12), color: palette.inkFaint)
                        } else {
                            Text(task).font(Theme.ui(12)).foregroundStyle(palette.inkFaint).lineLimit(1)
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    if let cost {
                        Text(cost).font(Theme.mono(10.5)).foregroundStyle(palette.inkFaint).lineLimit(1).fixedSize()
                    }
                    mark.frame(width: 14)
                }
                .padding(.leading, 10)
                .padding(.trailing, 11)
                .frame(minHeight: 36)
                .background(RoundedRectangle(cornerRadius: 10).fill(palette.raised.opacity(0.6)))
                .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(palette.border, lineWidth: 1))
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel(
                "Subagent \(subagent.name), \(subagentWords[subagent.state] ?? "")\(cost.map { ", \($0)" } ?? "")"
            )
            .accessibilityValue(open ? "expanded" : "collapsed")
            if open {
                Group {
                    if parts.isEmpty {
                        Text("No output yet.").font(Theme.ui(12.5)).foregroundStyle(palette.inkFaint).padding(
                            .vertical, 6)
                    } else {
                        Parts(id: "subagent/\(subagent.sessionId)", parts: parts, live: running)
                    }
                }
                .padding(.leading, 14)
                .padding(.top, 4)
            }
        }
        .padding(.vertical, 6)
    }

    @ViewBuilder private var mark: some View {
        switch subagent.state {
        case .running: Working()
        case .completed: IconView(icon: Icons.check, size: 13).foregroundStyle(palette.live)
        case .failed: IconView(icon: Icons.warning, size: 12).foregroundStyle(palette.danger)
        case .disconnected: IconView(icon: Icons.plug, size: 12).foregroundStyle(palette.danger)
        default: IconView(icon: Icons.close, size: 11).foregroundStyle(palette.inkFaint)
        }
    }

    static func cost(_ subagent: SubagentCard) -> String? {
        var pieces: [String] = []
        if let tokens = subagent.tokens, tokens > 0 { pieces.append("\(Format.tokens(tokens)) tokens") }
        if subagent.calls > 0 { pieces.append(ChatWords.callCount(subagent.calls)) }
        return pieces.isEmpty ? nil : pieces.joined(separator: " · ")
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

/// A trigger's prompt where the person's own messages sit, under a line naming the trigger, with the webhook request
/// that fired it folded beneath.
struct FiredTrigger: View {
    @Environment(\.palette) private var palette
    @Environment(\.displayScale) private var scale
    @Environment(TranscriptContext.self) private var context
    let id: String
    let fired: TriggerFired

    var body: some View {
        let open = context.folds.isOpen("\(id)/request", false)
        VStack(alignment: .trailing, spacing: 0) {
            HStack(spacing: 5) {
                IconView(icon: Icons.timer, size: 12).foregroundStyle(palette.cmd)
                Text("Trigger · \(Text(fired.title).font(Theme.ui(11.5, .medium)).foregroundStyle(palette.ink))")
                    .font(Theme.ui(11.5)).foregroundStyle(palette.inkDim).lineLimit(1)
            }
            .padding(.bottom, 6)
            Text(fired.prompt)
                .sentWords(palette).textSelection(.enabled)
                .sentBubble(fill: palette.raised, border: palette.border)
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
        .frame(maxWidth: .infinity, alignment: .trailing)
        .padding(.top, 14)
        .padding(.bottom, 6)
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

/// The line under the last row while the agent works: its mark breathing, what it is doing, and for how long.
struct ActivityLine: View {
    @Environment(\.palette) private var palette
    @Environment(\.accessibilityReduceMotion) private var still
    let provider: String
    let label: String
    let since: Date
    @State private var breath = false

    var body: some View {
        HStack(spacing: 9) {
            AgentMark(provider: provider, size: 20)
                .opacity(breath ? 0.5 : 1)
                .onAppear {
                    guard !still else { return }
                    withAnimation(.easeInOut(duration: 0.75).repeatForever()) { breath = true }
                }
            Shimmer(text: label, font: Theme.ui(12.5), color: palette.inkFaint)
            TimelineView(.periodic(from: since, by: 1)) { timeline in
                let seconds = Int(timeline.date.timeIntervalSince(since).rounded())
                if seconds > 0 {
                    Text(ChatWords.duration(ms: Double(seconds) * 1000)).font(Theme.mono(10)).foregroundStyle(
                        palette.inkFaint)
                }
            }
        }
        .padding(.top, 14)
        .padding(.bottom, 6)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(label)
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
                        "Couldn't load earlier messages ·\(Text(" Retry").font(Theme.ui(12.5, .medium)).foregroundStyle(palette.ink))"
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
                            .sentBubble(fill: palette.raised, border: palette.border)
                    }
                }
                .opacity(0.55)
                .padding(.bottom, 6)
            }
            Text("Sends when this turn ends").font(Theme.ui(11)).foregroundStyle(palette.inkFaint).padding(.top, 4)
        }
        .frame(maxWidth: .infinity, alignment: .trailing)
        .padding(.top, 14)
        .padding(.bottom, 6)
    }
}

extension String {
    var nilIfEmpty: String? { isEmpty ? nil : self }
}
