import SikemuxCore
import SwiftUI

/// What the bottom of a chat needs from the screen around it.
struct ChatBottomContext {
    var core: String
    var agentId: String
    var provider: String
    var hostName: String
    var watchOnly: Bool
    var offline: Bool
    /// Sending mid-turn steers the running turn rather than waiting behind it.
    var steers: Bool
    /// The live work strip docked on the composer, while there is work to show.
    var strip: AnyView?
    /// Scrolls the transcript to its end, once a message is sent.
    var onSent: () -> Void
    /// Opens the messages sent before, when there are any.
    var onRecent: (() -> Void)?
    var focus: ComposerFocus
}

struct ChatScreen: View {
    let core: String
    let agentId: String
    var started = false

    var body: some View {
        ChatPane(core: core, agentId: agentId)
            .id("\(core):\(agentId)")
    }
}

/// How long a message the person jumped to stays lit.
private let litFor: Duration = .milliseconds(900)

private struct ChatPane: View {
    @Environment(\.palette) private var palette
    let core: String
    let agentId: String
    @State private var session: ChatSession
    @State private var context: TranscriptContext
    @State private var scroll = TranscriptScroll()
    @State private var watch: HostWatch?
    @State private var release: (() -> Void)?
    @StateObject private var focus = ComposerFocus()
    /// The message whose strip is shown.
    @State private var open: String?
    @State private var editing: String?
    @State private var lit: String?
    @State private var menu = false
    @State private var showLive = false

    init(core: String, agentId: String) {
        self.core = core
        self.agentId = agentId
        let session = ChatSession.of(core: core, agentId: agentId)
        let info = Hub.shared.live(core).view?.chats.first { $0.agentId == agentId }
        _session = State(initialValue: session)
        _context = State(initialValue: TranscriptContext(
            provider: info?.provider ?? "agent", core: core, agentId: agentId, folds: session.folds,
            toolDetail: { [weak session] id, done in
                guard let session else { throw ChatProblem("the chat is closed") }
                return try await session.toolDetail(id, done: done)
            }))
    }

    private var live: HostLive { Hub.shared.live(core) }
    private var info: ChatInfo? { live.view?.chats.first { $0.agentId == agentId } }
    private var device: PairedDevice? { Hub.shared.device(core) }
    private var hostName: String { device?.displayName ?? "the host" }
    private var watchOnly: Bool { device?.access == "watch" }
    private var provider: String { info?.provider ?? "agent" }
    private var listed: AgentShown? { live.view?.inbox.agents.first { $0.agentId == agentId } }

    var body: some View {
        let rows = session.rows
        let title = rows.head.title ?? listed?.title ?? provider
        Screen {
            Nav(back: nil, onBack: nil) {
                AgentMark(provider: provider, size: 17)
                Text(title).font(Theme.ui(16, .semibold)).tracking(-0.25).foregroundStyle(palette.ink).lineLimit(1)
                    .accessibilityAddTraits(.isHeader)
                if let name = info?.name { Text("@\(name)").font(Theme.ui(12)).foregroundStyle(palette.tertiary).fixedSize() }
            } end: {
                Button { menu = true } label: {
                    IconView(icon: Icons.moreVertical, size: 16).foregroundStyle(palette.tertiary).frame(width: 40, height: 40)
                        .contentShape(Circle())
                }
                .buttonStyle(ActiveFill(radius: 20))
                .accessibilityLabel("Chat actions")
            }
            ZStack(alignment: .bottom) {
                if session.attached != .live && rows.messages.isEmpty {
                    VStack { Spacer(); status }.frame(maxWidth: .infinity)
                } else {
                    transcript(rows)
                        .overlay(alignment: .top) { askedStrip(rows) }
                        .overlay(alignment: .bottom) { jump }
                }
            }
            .frame(maxHeight: .infinity)
            .safeAreaInset(edge: .bottom, spacing: 0) { bottom(rows) }
        }
        .environment(context)
        .onAppear(perform: appeared)
        .onDisappear(perform: left)
        .onChange(of: info) { _, info in session.hostSaw(info) }
        .onChange(of: live.connection === nil) { _, _ in
            release?()
            release = session.hold(live.connection)
        }
        .onChange(of: rows.subagentRows) { _, rows in context.subagentRows = rows }
        .onChange(of: rows.running) { was, running in
            if was && !running { Haptics.success() }
        }
        .onChange(of: session.permissions.first?.requestId) { _, asked in
            if asked != nil { Haptics.warning() }
        }
        .onChange(of: lit) { _, shown in
            guard shown != nil else { return }
            Task {
                try? await Task.sleep(for: litFor)
                lit = nil
            }
        }
    }

    private func appeared() {
        watch = Hub.shared.watch(core)
        session.hostSaw(info)
        release = session.hold(live.connection)
        context.subagentRows = session.rows.subagentRows
        let actions = context.actions
        actions.tap = { [self] message in
            let owner = Strip.owner(session.rows.messages, message.id)
            open = open == owner ? nil : owner
        }
        actions.retry = { [weak session] id in session?.outbox.retry(id) }
        actions.edit = { [self] message in
            open = nil
            editing = message.id
        }
        actions.toggleWork = { [weak session] id in
            guard let folds = session?.folds else { return }
            folds.set("work/\(id)", !folds.isOpen("work/\(id)", false))
        }
        actions.toggleRun = { [weak session, scroll] id in
            guard let folds = session?.folds else { return }
            scroll.hold()
            folds.set("team/\(id)", !folds.isOpen("team/\(id)", false))
            folds.set("team-all/\(id)", false)
        }
        actions.showRunEarlier = { [weak session, scroll] id in
            scroll.hold()
            session?.folds.set("team-all/\(id)", true)
        }
    }

    private func left() {
        release?()
        release = nil
        watch?.cancel()
        watch = nil
        session.keep()
    }

    // MARK: The transcript

    private func transcript(_ rows: ChatRows) -> some View {
        let shown = shownRows(rows)
        return TranscriptList(
            order: shown.order, entries: shown.entries, drawn: session.drawn, touched: { session.takeTouched() },
            scroll: scroll, hasEarlier: session.hasEarlier && session.earlier == .idle, loadEarlier: { session.loadEarlier() },
            environment: { [palette, context] view in AnyView(view.environment(\.palette, palette).environment(context)) },
            header: { TranscriptHeader(session: session) },
            footer: { TranscriptFooter(session: session, core: core, agentId: agentId, provider: provider) },
            row: { entry in TranscriptRow(entry: entry) })
            .environment(\.openURL, OpenURLAction { url in opened(url) })
            .sheet(item: $context.picture) { picture in ImageViewer(image: picture.image, name: picture.name) }
    }

    private func opened(_ url: URL) -> OpenURLAction.Result {
        guard let path = TranscriptLink.filePath(url) else { return .systemAction }
        if let image = HostFiles.imagePath(path) {
            Task {
                guard let file = await HostFiles.shared.read(core: core, path: image), let picture = UIImage(contentsOfFile: file.path) else { return }
                context.showImage(picture, (image as NSString).lastPathComponent)
            }
        }
        return .handled
    }

    private func shownRows(_ rows: ChatRows) -> (order: [String], entries: [String: RowEntry]) {
        let folds = session.folds
        let messages = rows.messages
        let runs = ShownRows.teamRows(messages) { id in
            folds.isOpen("team-all/\(id)", false) ? .all : folds.isOpen("team/\(id)", false) ? .open : .closed
        }
        let liveId = rows.running ? messages.last?.id : nil
        let canEdit = !watchOnly && rows.head.capabilities.editing == true && rows.head.connection == .ready && !rows.running && session.attached == .live
        let stripOwner = open
        let worktree = live.view?.workspace.agents?.first { $0.id == agentId }?.worktree
        var order: [String] = []
        var entries: [String: RowEntry] = [:]
        for (index, message) in messages.enumerated() {
            let run = runs[message.id]
            let fold = ShownRows.fold(message, { folds.isOpen("work/\($0)", false) }, run)
            guard !fold.hidden || fold.summary != nil || run?.head != nil else { continue }
            var look = RowLook()
            look.live = message.id == liveId
            look.lit = message.id == lit
            look.unsent = session.unsent[message.id]
            look.kept = fold.kept
            look.hidden = fold.hidden
            look.summary = fold.summary
            look.workOpen = fold.summary.map { folds.isOpen("work/\($0.id)", false) } ?? false
            if let run, let head = run.head {
                look.team = head
                look.teamId = run.id
                look.teamOpen = folds.isOpen("team/\(run.id)", false)
                look.teamAll = folds.isOpen("team-all/\(run.id)", false)
            }
            if index == 0, !session.hasEarlier { look.worktree = worktree }
            if message.id == stripOwner { look.strip = Strip.of(messages, message.id) }
            look.editable = canEdit && message.role == .user && message.promptId != nil && (message.context ?? []).isEmpty && session.unsent[message.id] == nil
            look.editing = message.id == editing && canEdit
            order.append(message.id)
            entries[message.id] = RowEntry(message: message, look: look)
        }
        return (order, entries)
    }

    @ViewBuilder private func askedStrip(_ rows: ChatRows) -> some View {
        if let asked = ShownRows.lastAsked(rows.messages), let top = scroll.topRow,
           let askedAt = rows.messages.firstIndex(where: { $0.id == asked.id }),
           let topAt = rows.messages.firstIndex(where: { $0.id == top }), topAt > askedAt {
            let text = ShownRows.askedText(asked)
            Button {
                Haptics.select()
                lit = asked.id
                scroll.reveal(asked.id)
            } label: {
                HStack(spacing: 8) {
                    Text("You asked").font(Theme.ui(12)).foregroundStyle(palette.inkDim)
                    Text(text).font(Theme.ui(12)).foregroundStyle(palette.ink).lineLimit(1).frame(maxWidth: .infinity, alignment: .leading)
                }
                .padding(.horizontal, 18)
                .frame(height: 32)
                .background(palette.raised)
                .overlay(alignment: .bottom) { Rectangle().fill(palette.border).frame(height: 1 / UIScreen.main.scale) }
            }
            .buttonStyle(.plain)
            .accessibilityLabel("You asked: \(text). Go to it")
        }
    }

    @ViewBuilder private var jump: some View {
        if scroll.away {
            Button { scroll.toEnd() } label: {
                IconView(icon: Icons.arrowDown, size: 16)
                    .foregroundStyle(palette.inkDim)
                    .frame(width: 34, height: 34)
                    .background(Circle().fill(palette.composer))
                    .overlay(Circle().strokeBorder(palette.border, lineWidth: 1))
                    .shadow(color: .black.opacity(0.4), radius: 12, y: 6)
            }
            .buttonStyle(.plain)
            .padding(.bottom, 8)
            .accessibilityLabel("Jump to latest message")
        }
    }

    // MARK: Around the transcript

    @ViewBuilder private var status: some View {
        let unreachable = session.attached != .live && isClosed
        if unreachable {
            ChatProblemView(text: "Can't reach \(hostName). \(capitalised(closedProblem))", action: "Try again") { Hub.shared.retry(core) }
        } else if session.attached == .missing {
            ChatProblemView(text: session.problem.map(capitalised) ?? "This chat is no longer running on the host.", action: "Try again") { session.retry() }
        } else if session.attached == .attaching {
            HStack(spacing: 10) {
                Working()
                Text(isConnecting ? "Reaching \(hostName)…" : info?.state == .asleep ? "Waking the chat…" : "Opening the chat…")
                    .metaStyle(palette)
            }
            .padding(.vertical, 24)
            .accessibilityElement(children: .combine)
        }
    }

    private var isClosed: Bool {
        if case .closed = live.status { return true }
        return false
    }

    private var isConnecting: Bool {
        if case .connecting = live.status { return true }
        return false
    }

    private var closedProblem: String {
        if case .closed(let problem, _, _) = live.status { return problem }
        return ""
    }

    @ViewBuilder private func bottom(_ rows: ChatRows) -> some View {
        if session.attached == .live {
            VStack(spacing: 0) {
                if !live.isOpen { Reconnecting(closed: isClosed, problem: closedProblem, hostName: hostName) { Hub.shared.retry(core) } }
                if let notice = session.notice { NoticeBar(text: notice) { session.dismissNotice() } }
                ChatBottom(session: session, context: bottomContext(rows))
            }
        } else if !rows.messages.isEmpty {
            status
                .frame(maxWidth: .infinity)
                .overlay(alignment: .top) { Rectangle().fill(palette.border).frame(height: 1) }
        }
    }

    private func bottomContext(_ rows: ChatRows) -> ChatBottomContext {
        let steerable = rows.head.capabilities.steering == true && rows.running && !watchOnly
        return ChatBottomContext(
            core: core, agentId: agentId, provider: provider, hostName: hostName, watchOnly: watchOnly,
            offline: !live.isOpen, steers: steerable, strip: nil, onSent: { scroll.toEnd() }, onRecent: nil, focus: focus)
    }
}

/// Above the first row: earlier turns arriving, or a note that the host no longer keeps them.
private struct TranscriptHeader: View {
    let session: ChatSession

    var body: some View {
        if session.hasEarlier {
            EarlierLine(failed: session.earlier == .failed, loading: session.earlier == .loading) { session.loadEarlier() }
        } else if session.rows.head.earlierLost {
            Text(ChatWords.earlierLost).font(Theme.ui(12.5)).foregroundStyle(Palette.standard.tertiary)
                .frame(maxWidth: .infinity).padding(.top, 10).padding(.bottom, 6)
        } else {
            Color.clear.frame(height: 6)
        }
    }
}

/// Below the last row: what waits to be sent, what the agent is doing, what went wrong, and questions.
private struct TranscriptFooter: View {
    let session: ChatSession
    let core: String
    let agentId: String
    let provider: String

    var body: some View {
        let rows = session.rows
        let activity = session.stoppingTurn ? "Stopping…" : session.permissions.isEmpty ? ChatWords.activity(rows.head.activity) : nil
        VStack(alignment: .leading, spacing: 0) {
            if !rows.queued.isEmpty { QueuedLines(held: rows.queued) }
            if let activity { ActivityLine(provider: provider, label: activity, since: session.turnSince) }
            ChatQuestionsFooter(core: core, agentId: agentId, session: session)
        }
        .padding(.bottom, 28)
    }
}

func capitalised(_ text: String) -> String {
    guard let first = text.first else { return text }
    return first.uppercased() + text.dropFirst() + (text.hasSuffix(".") ? "" : ".")
}

private struct ChatProblemView: View {
    @Environment(\.palette) private var palette
    let text: String
    let action: String
    let onAction: () -> Void

    var body: some View {
        VStack(spacing: 16) {
            Text(text).metaStyle(palette).multilineTextAlignment(.center)
            ActionButton(action, action: onAction)
        }
        .padding(.horizontal, 18)
        .padding(.vertical, 24)
    }
}

/// Above the composer while the host is out of reach: the chat stays, and what is sent waits.
private struct Reconnecting: View {
    @Environment(\.palette) private var palette
    let closed: Bool
    let problem: String
    let hostName: String
    let onTry: () -> Void

    var body: some View {
        HStack(spacing: 10) {
            if !closed { Working() }
            Text(closed ? "Can't reach \(hostName). \(capitalised(problem))" : "Reconnecting to \(hostName)…")
                .font(Theme.ui(13)).foregroundStyle(palette.secondary).lineLimit(2)
                .frame(maxWidth: .infinity, alignment: .leading)
            Button("Try now", action: onTry).font(Theme.ui(13, .semibold)).foregroundStyle(palette.ink)
                .accessibilityLabel("Try reaching \(hostName) now")
        }
        .padding(.vertical, 9)
        .padding(.horizontal, 12)
        .background(RoundedRectangle(cornerRadius: 12).fill(palette.overlay))
        .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(palette.border, lineWidth: 1))
        .padding(.horizontal, 10)
        .padding(.top, 8)
    }
}

/// Something this phone asked for that did not happen, said where the person is looking.
private struct NoticeBar: View {
    @Environment(\.palette) private var palette
    let text: String
    let onDismiss: () -> Void

    var body: some View {
        HStack(spacing: 9) {
            IconView(icon: Icons.warning, size: 13).foregroundStyle(palette.danger)
            Text(capitalised(text)).font(Theme.ui(13)).foregroundStyle(palette.secondary).frame(maxWidth: .infinity, alignment: .leading)
            Button(action: onDismiss) {
                IconView(icon: Icons.close, size: 13).foregroundStyle(palette.inkDim)
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Dismiss")
        }
        .padding(.vertical, 9)
        .padding(.horizontal, 12)
        .background(RoundedRectangle(cornerRadius: 12).fill(palette.overlay))
        .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(palette.border, lineWidth: 1))
        .padding(.horizontal, 10)
        .padding(.top, 8)
    }
}
