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
    @State private var finding = false
    @State private var stillness: (() -> Void)?

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
                ChatTitle(agentId: agentId, provider: provider, name: info?.name, title: title)
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
                        .overlay(alignment: .bottom) { JumpToLatest(scroll: scroll) }
                }
            }
            .frame(maxHeight: .infinity)
            .safeAreaInset(edge: .bottom, spacing: 0) { bottom(rows) }
        }
        .environment(context)
        .onAppear(perform: appeared)
        .onDisappear(perform: left)
        .onChange(of: info) { _, info in session.hostSaw(info) }
        .onChange(of: live.isOpen) { _, _ in
            release?()
            release = session.hold(live.connection)
        }
        .sheet(shown: $showLive) {
            LiveSheet(work: LiveWork(rows), provider: provider, steerable: steerable(rows), canAct: !watchOnly, session: session) { scroll.toEnd() }
        }
        .sheet(shown: $menu) {
            ChatMenu(
                core: core, agentId: agentId, agentName: info?.name.map { "@\($0)" } ?? provider, hostName: hostName,
                onClose: { menu = false },
                onFind: session.attached == .live ? { finding = true } : nil,
                onRecent: recalls(rows) ? { focus.recent = true } : nil,
                onStop: watchOnly || session.attached != .live ? nil : { await session.stopChat() })
        }
        .onChange(of: rows.subagentRows) { _, rows in context.subagentRows = rows }
        .onChange(of: rows.running, initial: true) { was, running in
            if was && !running { Haptics.success() }
            // A streaming reply needs the main thread more than the moving backdrop does.
            stillness?()
            stillness = running ? BackdropPause.shared.hold() : nil
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
        Cards.dismiss(host: core, agent: agentId)
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
        stillness?()
        stillness = nil
        release?()
        release = nil
        watch?.cancel()
        watch = nil
        session.keep()
    }

    // MARK: The transcript

    private func transcript(_ rows: ChatRows) -> some View {
        let canEdit = !watchOnly && rows.head.capabilities.editing == true && rows.head.connection == .ready && !rows.running && session.attached == .live
        let messages = rows.messages
        let unsent = session.unsent
        return Transcript(
            rows: rows, drawn: session.drawn, touched: { session.takeTouched() }, context: context, scroll: scroll,
            hasEarlier: session.hasEarlier, earlier: session.earlier, loadEarlier: { session.loadEarlier() },
            worktree: live.view?.workspace.agents?.first { $0.id == agentId }?.worktree,
            look: { [open, lit, editing] message, look in
                look.lit = message.id == lit
                look.unsent = unsent[message.id]
                if message.id == open { look.strip = Strip.of(messages, message.id) }
                look.editable = canEdit && message.role == .user && message.promptId != nil && (message.context ?? []).isEmpty && unsent[message.id] == nil
                look.editing = message.id == editing && canEdit
            },
            footer: { TranscriptFooter(session: session, core: core, agentId: agentId, provider: provider, hostName: hostName, canAct: !watchOnly, focus: focus) })
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
        let work = LiveWork(rows)
        let strip = work.any && session.attached == .live ? AnyView(LiveStrip(work: work, provider: provider) { showLive = true }) : nil
        return ChatBottomContext(
            core: core, agentId: agentId, provider: provider, hostName: hostName, watchOnly: watchOnly,
            offline: !live.isOpen, steers: steerable(rows), strip: strip, onSent: { scroll.toEnd() }, onRecent: nil, focus: focus)
    }

    private func recalls(_ rows: ChatRows) -> Bool {
        !watchOnly && (!rows.queued.isEmpty || rows.messages.contains { $0.role == .user })
    }

    private func steerable(_ rows: ChatRows) -> Bool {
        rows.head.capabilities.steering == true && rows.running && !watchOnly
    }
}

/// Below the last row: what waits to be sent, what the agent is doing, what went wrong, and questions.
private struct TranscriptFooter: View {
    let session: ChatSession
    let core: String
    let agentId: String
    let provider: String
    let hostName: String
    let canAct: Bool
    let focus: ComposerFocus

    var body: some View {
        let rows = session.rows
        let activity = session.stoppingTurn ? "Stopping…" : session.permissions.isEmpty ? ChatWords.activity(rows.head.activity) : nil
        VStack(alignment: .leading, spacing: 0) {
            if !rows.queued.isEmpty { QueuedLines(held: rows.queued) }
            if let activity { ActivityLine(provider: provider, label: activity, since: session.turnSince) }
            Trouble(session: session, provider: provider, hostName: hostName, canAct: canAct && session.attached == .live && Hub.shared.live(core).isOpen)
            ChatQuestionsFooter(core: core, agentId: agentId, session: session, canAnswer: canAct, focus: focus)
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
