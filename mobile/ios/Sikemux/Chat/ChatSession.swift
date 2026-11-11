import Foundation
import Observation
import SikemuxCore
import UIKit

/// One chat on one host, kept apart from the screen so it outlives it: the chat as the host folds it, where its
/// changes got to, what this phone sent and what it still means to send.
@MainActor @Observable
final class ChatSession {
    enum Attached { case attaching, live, missing }

    /// Every chat this phone opened since it started, so going back to one finds it as it was left.
    @ObservationIgnored private static var sessions: [String: ChatSession] = [:]

    static func of(core: String, agentId: String) -> ChatSession {
        let key = "\(core):\(agentId)"
        if let found = sessions[key] { return found }
        let made = ChatSession(core: core, agentId: agentId)
        sessions[key] = made
        return made
    }

    let core: String
    let agentId: String
    let folds = Folds()

    /// The chat as the host folds it, with what this phone sent and picked that the host does not show yet.
    private(set) var rows = ChatRows.empty
    /// The permission requests still waiting on the host.
    private(set) var permissions: [PermissionAsk] = []
    private(set) var unsent: [String: Unsent] = [:]
    /// What this phone sent, by the path the host keeps it at.
    private(set) var sentFiles: [String: Attachment] = [:]
    private(set) var hasEarlier = false
    private(set) var earlier = Feed.Earlier.idle
    /// When the running turn began.
    private(set) var turnSince = Date()
    /// Counts each change to the rows, so the transcript knows to look at `touched`.
    private(set) var drawn = 0

    var attached = Attached.attaching
    /// Why the chat could not be opened, in the host's words.
    var problem: String?
    /// Something this phone asked for that did not happen, such as a stop or a setting.
    var notice: String?
    /// Permission requests whose answer is on its way.
    var answering = Set<String>()
    /// Background tasks this phone asked the host to stop.
    var stopping = Set<String>()
    /// This phone asked the running turn to stop and the host has not ended it yet.
    var stoppingTurn = false
    /// The mode this phone set, until the host says another.
    var mode: PermissionMode?
    /// Model and effort picks this phone made, by picker id, shown until the host says what it runs on.
    var chosen: [String: String] = [:]

    @ObservationIgnored let feed: Feed
    @ObservationIgnored private(set) var draft: ComposerDraft!
    @ObservationIgnored private(set) var outbox: Outbox!
    @ObservationIgnored private(set) var connection: Connection?
    @ObservationIgnored private var holders = 0
    /// Counts attaches, so an answer that comes back after the chat was taken up again is dropped.
    @ObservationIgnored private var run = 0
    /// Reading the chat this phone kept, which the first attach waits for so it asks only for what came after.
    @ObservationIgnored private var restoring: Task<Void, Never>?
    @ObservationIgnored private var stopRun: (() -> Void)?
    /// What the host lists as up, or nil before it has said.
    @ObservationIgnored private var hostUp: Bool?
    /// When this phone saw the running turn begin, for a turn the host has no time for.
    @ObservationIgnored private var sawTurnBegin = Date()

    private init(core: String, agentId: String) {
        self.core = core
        self.agentId = agentId
        feed = Feed(store: KeptChat(core: core, agentId: agentId))
        feed.applied = { [unowned self] before in applied(before) }
        feed.emit = { [unowned self] in emit() }
        feed.fellBehind = { [unowned self] in restart() }
        draft = ComposerDraft(
            core: core, agentId: agentId,
            held: { [unowned self] in feed.chat.head.draft },
            live: { [unowned self] in connection?.isOpen() == true && attached == .live ? connection : nil },
            emit: { [unowned self] in emit() })
        outbox = Outbox(
            agentId: agentId,
            connection: { [unowned self] in connection },
            chat: { [unowned self] in feed.chat },
            emit: { [unowned self] in emit() })
        restoring = Task { [weak self] in
            guard let self, await feed.restore() else { return }
            attached = .live
            draft.takeIn()
            emit()
        }
        emit()
        NotificationCenter.default.addObserver(
            forName: UIApplication.didEnterBackgroundNotification, object: nil, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated {
                self?.keep()
                self?.draft.keep()
            }
        }
    }

    // MARK: What the screen reads

    var permissionMode: PermissionMode? { mode ?? rows.permissionMode }
    var mark: ChatMark? { feed.mark }

    /// Whether the transcript must redraw everything, or which rows, since it last asked; asking clears it.
    func takeTouched() -> Touched {
        defer { feed.touched = Touched() }
        return feed.touched
    }

    func emit() {
        let chat = feed.chat
        var view = outbox.over(chat)
        if !chosen.isEmpty {
            view.setHead { head in
                if let model = head.model, let value = chosen[model.id] { head.model?.current = value }
                if let effort = head.effort, let value = chosen[effort.id] { head.effort?.current = value }
            }
        }
        rows = view
        permissions = chat.head.permissions.filter { !answering.contains($0.requestId) }
        unsent = outbox.unsent
        sentFiles = outbox.sentFiles
        hasEarlier = chat.olderBefore != nil
        earlier = feed.earlier
        turnSince = since(view)
        drawn += 1
    }

    /// When the running turn began: as the host timed it, or else as this phone saw it.
    private func since(_ view: ChatRows) -> Date {
        if let at = view.head.turnOpening?.at { return Format.date(millis: at) }
        for row in view.messages.reversed() {
            if row.role == .user {
                return row.sentAt.map { Format.date(millis: $0) } ?? sawTurnBegin
            }
            if let opens = row.opensTurn {
                return opens.at.map { Format.date(millis: $0) } ?? sawTurnBegin
            }
        }
        return sawTurnBegin
    }

    // MARK: Attaching

    /// Keeps the chat attached over this connection until the returned function is called.
    func hold(_ connection: Connection?) -> () -> Void {
        guard let connection else { return {} }
        holders += 1
        if self.connection !== connection {
            stop()
            self.connection = connection
            start()
        }
        return { [weak self] in
            guard let self else { return }
            holders -= 1
            guard holders == 0, self.connection === connection else { return }
            stop()
            self.connection = nil
        }
    }

    /// What the host's own view says of this chat. A chat that comes back after it slept, stopped or restarted is
    /// taken up again.
    func hostSaw(_ info: ChatInfo?) {
        let up = info?.state == .ready
        let cameBack = hostUp == false && up
        hostUp = up
        if cameBack, connection != nil, attached != .attaching { restart() }
    }

    /// Opens the chat again from the start, after it could not be opened.
    func retry() {
        feed.forget()
        if connection != nil { restart() }
    }

    /// Writes the chat down as drawn, unless it has not changed since it last was.
    func keep() {
        feed.keep()
    }

    private func restart() {
        stop()
        start()
    }

    private func stop() {
        run += 1
        stopRun?()
        stopRun = nil
        feed.cut()
        feed.keep()
    }

    private func start() {
        guard let connection else { return }
        run += 1
        let run = run
        if feed.mark == nil || attached == .missing { attached = .attaching }
        problem = nil
        emit()
        var held: [ChatDelivery]? = []
        let listening = Hub.shared.onChatEvents(core) { [weak self] deliveries in
            guard let self, run == self.run else { return }
            let mine = deliveries.filter { $0.agentId == self.agentId }
            guard !mine.isEmpty else { return }
            // Until the attach answer says where the chat it sends stands, changes wait.
            if held != nil { held?.append(contentsOf: mine) } else { feed.take(mine) }
        }
        let agentId = agentId
        stopRun = { [weak self] in
            listening.cancel()
            // What arrived but was not applied yet goes into the chat now, so the next attach picks up after it.
            self?.feed.applyPending()
            guard connection.isOpen() else { return }
            Task {
                do {
                    try await connection.detachChat(agentId: agentId)
                } catch {
                    print("sikemux: could not detach \(agentId): \(error)")
                }
            }
        }
        let up = hostUp == true
        let restoring = restoring
        Task { [weak self] in
            await restoring?.value
            guard let current = self, run == current.run else { return }
            let began = Date()
            let mark = current.feed.mark
            var woke = false
            do {
                var attachment: ChatAttachment
                // A chat the host says is up is taken up at once; one asleep, or gone since the host said so, is
                // started first.
                if up {
                    attachment = try await connection.attachChat(agentId: agentId, since: mark)
                    if case .missing = attachment {
                        woke = true
                        try await connection.wakeChat(agentId: agentId)
                        attachment = try await connection.attachChat(agentId: agentId, since: mark)
                    }
                } else {
                    woke = true
                    try await connection.wakeChat(agentId: agentId)
                    attachment = try await connection.attachChat(agentId: agentId, since: mark)
                }
                let answered = Date().timeIntervalSince(began)
                let decoded = try await Self.read(attachment)
                guard let self, run == self.run else { return }
                attachedWith(decoded, held: held ?? [])
                held = nil
                Self.logOpen(agentId, woke: woke, answered: answered, decoded, began)
            } catch {
                guard let self, run == self.run else { return }
                problem = Sikemux.problem(error)
                attached = .missing
                outbox.failWaiting(problem ?? "")
                emit()
            }
        }
    }

    private enum Opened {
        case live(ChatView, ChatMark, Int)
        case resumed([ChatChange], ChatMark, Int)
        case missing
    }

    /// Reads what the attach answered off the main thread; a long chat's view is megabytes of JSON.
    private nonisolated static func read(_ attachment: ChatAttachment) async throws -> Opened {
        try await Task.detached(priority: .userInitiated) {
            switch attachment {
            case .live(let json, let mark):
                return .live(try JSONDecoder().decode(ChatView.self, from: Data(json.utf8)), mark, json.utf8.count)
            case .resumed(let json, let mark):
                return .resumed(
                    try JSONDecoder().decode([ChatChange].self, from: Data(json.utf8)), mark, json.utf8.count)
            case .missing:
                return .missing
            }
        }.value
    }

    private func attachedWith(_ opened: Opened, held: [ChatDelivery]) {
        switch opened {
        case .live(let view, let mark, _):
            feed.open(at: view, mark)
        case .resumed(let changes, let mark, _):
            feed.resume(at: changes, mark)
        case .missing:
            attached = .missing
            // The chat kept on this phone may hold requests nothing on the host waits on any more.
            feed.apply { chat, touched in
                chat.setHead { $0.permissions = [] }
                touched.head = true
            }
            outbox.failWaiting("the chat is gone from the host")
            emit()
            return
        }
        hostUp = true
        attached = .live
        feed.take(held)
        outbox.pushWaiting()
        Task { await draft.save() }
        emit()
    }

    private static func logOpen(_ agentId: String, woke: Bool, answered: TimeInterval, _ opened: Opened, _ began: Date)
    {
        #if DEBUG
            let (answer, bytes): (String, Int) =
                switch opened {
                case .live(_, _, let bytes): ("live", bytes)
                case .resumed(_, _, let bytes): ("resumed", bytes)
                case .missing: ("missing", 0)
                }
            let ms = { (seconds: TimeInterval) in "\(Int(seconds * 1000))ms" }
            print(
                "sikemux: \(agentId) \(woke ? "woke and " : "")\(answer) after \(ms(answered)) with \(bytes / 1024) KB, applied by \(ms(Date().timeIntervalSince(began)))"
            )
        #endif
    }

    /// What follows from the host's changes, and letting go of what this phone sent once the host shows it.
    private func applied(_ before: ChatRows) {
        let chat = feed.chat
        if !before.running && chat.running { sawTurnBegin = Date() }
        if !chat.running { stoppingTurn = false }
        if chat.permissionMode != before.permissionMode { mode = nil }
        if chat.head.queued != before.head.queued || chat.running != before.running { outbox.dropTaken() }
        if !chosen.isEmpty && (chat.head.model != before.head.model || chat.head.effort != before.head.effort) {
            chosen = [:]
        }
        outbox.settle(turnEnded: before.running && !chat.running)
        if chat.head.draft != before.head.draft { draft.takeIn() }
    }

    // MARK: Asking the host

    func openConnection() throws -> Connection {
        guard let connection, connection.isOpen(), attached == .live else {
            throw ChatProblem.notConnected
        }
        return connection
    }

    /// Runs a request on the open connection; on failure says `what` went wrong where the person is looking and
    /// runs `undo`.
    @discardableResult
    func ask<T>(
        _ what: String, then: ((T) -> Void)? = nil, undo: (() -> Void)? = nil,
        _ request: @escaping (Connection) async throws -> T
    ) async -> Bool {
        do {
            guard let connection, connection.isOpen() else { throw ChatProblem.notConnected }
            let answer = try await request(connection)
            then?(answer)
            emit()
            return true
        } catch {
            undo?()
            notice = "\(what): \(Sikemux.problem(error))"
            emit()
            return false
        }
    }

    /// Says what went wrong where the person is looking.
    func report(_ text: String) {
        notice = text
    }

    func dismissNotice() {
        notice = nil
    }

    var busy: Bool { feed.chat.running }

    // MARK: The transcript's own requests

    func stopTask(_ taskId: String) {
        guard !stopping.contains(taskId) else { return }
        stopping.insert(taskId)
        notice = nil
        let agentId = agentId
        Task {
            await ask("Could not stop the task") { try await $0.stopTask(agentId: agentId, taskId: taskId) }
            stopping.remove(taskId)
        }
    }

    enum FailedTurn { case retry, discard, cancelResume }

    /// Acts on the turn that failed: sends it again, lets it go, or keeps it from resuming once a usage limit resets.
    func failedTurn(_ how: FailedTurn) {
        let params = ChatAccountsParams(agentId: agentId)
        Task {
            await ask("Could not do that") { connection in
                switch how {
                case .retry: _ = try await connection.call(PhoneCall.RetryFailed.self, params)
                case .discard: _ = try await connection.call(PhoneCall.DiscardFailed.self, params)
                case .cancelResume: _ = try await connection.call(PhoneCall.CancelResume.self, params)
                }
            }
        }
    }

    /// Carries the chat on in another of the person's accounts, a turn that failed sent again there.
    func moveToAccount(_ accountId: String) {
        let params = MoveChatParams(agentId: agentId, accountId: accountId)
        Task { await ask("Could not move the chat") { _ = try await $0.call(PhoneCall.MoveChatAccount.self, params) } }
    }

    /// Ends the chat's agent on the host and closes it in every app. Answers whether it did.
    func stopChat() async -> Bool {
        let params = AgentParams(agentId: agentId)
        return await ask("Could not stop the chat") { _ = try await $0.call(PhoneCall.CloseChat.self, params) }
    }

    /// Takes the chat back to before the person's message `rowId` and sends `text` in its place with the files it
    /// carried. The message and everything after it leave at once; if the host refuses, the chat is read again whole.
    func edit(_ rowId: String, _ text: String) {
        guard let row = feed.chat.messages.first(where: { $0.id == rowId }), let messageId = row.promptId, !text.isEmpty
        else { return }
        notice = nil
        feed.apply { chat, touched in chat.apply([.drop(.init(from: rowId))], touched: &touched) }
        emit()
        let params = EditParams(agentId: agentId, messageId: messageId, text: text, paths: row.attachments ?? [])
        Task {
            await ask("Could not edit the message", undo: { self.retry() }) {
                _ = try await $0.call(PhoneCall.EditMessage.self, params)
            }
        }
    }

    /// Fetches the rows before the first one shown, unless that is already under way.
    func loadEarlier() {
        Task { _ = await feed.pageBack(connection, agentId) }
    }

    /// Pages back until row `id` is held, answering whether it is.
    func reach(_ id: String) async -> Bool {
        while !feed.chat.messages.contains(where: { $0.id == id }) {
            guard await feed.pageBack(connection, agentId) else { return false }
        }
        return true
    }

    /// Asks the host to find words across the whole chat, held here or not.
    func find(
        _ query: String, matchCase: Bool, wholeWord: Bool, from: FindAt?, direction: FindDirection?, limit: Int?,
        countOnly: Bool
    ) async throws -> FindAnswer {
        guard let feed = feed.mark?.feed else { throw ChatProblem("the chat is not open") }
        let params = FindParams(
            agentId: agentId, feed: feed, query: query, matchCase: matchCase, wholeWord: wholeWord, from: from,
            direction: direction, limit: limit, countOnly: countOnly)
        return try await openConnection().call(PhoneCall.ChatFind.self, params)
    }

    /// A call's whole change and what it printed, which rows carry only counts of. `done` calls are read once.
    func toolDetail(_ toolCallId: String, done: Bool) async throws -> ToolDetail {
        try await feed.toolDetail(connection, agentId, toolCallId, done: done)
    }
}

/// Which tool groups and rows in a transcript the person opened or closed, kept apart from the rows so a row the
/// list reuses comes back as it was left.
@MainActor @Observable
final class Folds {
    /// Messages the person watched stream in, which are never folded short.
    @ObservationIgnored private var streamed = Set<String>()
    private var open: [String: Bool] = [:]

    /// Whether the person watched `id` stream in, noting it while it is `live`.
    func watched(_ id: String, live: Bool) -> Bool {
        if live { streamed.insert(id) }
        return streamed.contains(id)
    }

    /// Whether a group or row is open; one nobody touched follows `otherwise`.
    func isOpen(_ id: String, _ otherwise: Bool) -> Bool { open[id] ?? otherwise }

    func set(_ id: String, _ value: Bool) { open[id] = value }

    /// A turn's folded work, a team's run, and that run with every member shown, as the chat screen and the
    /// transcript both open them.
    static func work(_ id: String) -> String { "work/\(id)" }
    static func team(_ id: String) -> String { "team/\(id)" }
    static func teamAll(_ id: String) -> String { "team-all/\(id)" }
}
