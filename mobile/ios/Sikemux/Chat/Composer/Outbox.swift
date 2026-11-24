import Foundation
import Observation
import SikemuxCore

/// Waiting offline messages kept on disk so they outlive app kills and send once on reconnect.
enum OutboxStore {
    struct Saved: Codable {
        var messageId: String
        var text: String
        var files: [Attachment]
        var paths: [String]
        var context: [ChatContext]
        var at: Int
        var steers: Bool
        var state: String?
        var problem: String?
    }

    private static var folder: URL {
        URL.applicationSupportDirectory.appending(path: "outbox", directoryHint: .isDirectory)
    }

    private static func file(core: String, agentId: String) -> URL {
        let key = "\(core):\(agentId)"
        return folder.appending(path: "\(key.addingPercentEncoding(withAllowedCharacters: .alphanumerics) ?? key).json")
    }

    static func load(core: String, agentId: String) -> [Saved] {
        let url = file(core: core, agentId: agentId)
        guard FileManager.default.fileExists(atPath: url.path),
            let data = try? Data(contentsOf: url),
            let entries = try? JSONDecoder().decode([Saved].self, from: data)
        else { return [] }
        return entries
    }

    static func save(core: String, agentId: String, entries: [Saved]) {
        let url = file(core: core, agentId: agentId)
        if entries.isEmpty {
            try? FileManager.default.removeItem(at: url)
            return
        }
        try? FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        try? JSONEncoder().encode(entries).write(to: url, options: .atomic)
    }

    static func listPending(for core: String) -> [String] {
        guard let files = try? FileManager.default.contentsOfDirectory(at: folder, includingPropertiesForKeys: nil)
        else { return [] }
        var agents: [String] = []
        for url in files where url.pathExtension == "json" {
            let encoded = url.deletingPathExtension().lastPathComponent
            guard let decoded = encoded.removingPercentEncoding,
                let sep = decoded.firstIndex(of: ":")
            else { continue }
            if String(decoded[..<sep]) == core {
                agents.append(String(decoded[decoded.index(after: sep)...]))
            }
        }
        return agents
    }

    static func forgetHost(_ core: String) {
        guard let files = try? FileManager.default.contentsOfDirectory(at: folder, includingPropertiesForKeys: nil)
        else { return }
        let prefix = "\(core):"
        for url in files where url.pathExtension == "json" {
            let encoded = url.deletingPathExtension().lastPathComponent
            if let decoded = encoded.removingPercentEncoding, decoded.hasPrefix(prefix) {
                try? FileManager.default.removeItem(at: url)
            }
        }
    }
}

/// What this phone sent that the host does not show yet, sent in order however long each one's files take.
@MainActor @Observable
final class Outbox {
    /// `sent` is taken by the host and not shown by it yet. Its `files` go up behind it, each named in the row by the
    /// host's path once it is there and by this phone's copy before. `waiting` was sent before the chat was attached,
    /// and goes out once it is.
    private struct Outgoing {
        enum State { case sending, failed, sent }
        let messageId: String
        let text: String
        var files: [Attachment]
        let named: Named
        let at: Int
        var state: State = .sending
        var problem: String?
        var waiting = false
        /// Kept on disk until the host acknowledges or settles it, so kills mid-upload or on failure never lose it.
        var kept = false
        var steers = false
    }

    /// A message sent behind the running turn, shown in the queue until the host's own queue holds it.
    private struct Holding {
        let message: QueuedMessage
        var taken = false
    }

    /// What this phone sent, by the path the host keeps it at.
    private(set) var sentFiles: [String: Attachment] = [:]
    private var entries: [Outgoing] = []
    private var holding: [Holding] = []
    /// Bumped when the host is forgotten or unpaired so in-flight rejections never re-persist or re-watch.
    @ObservationIgnored private var epoch = 0
    /// Messages reach the host in the order they were sent, however long their files take.
    @ObservationIgnored private var line: Task<Void, Never>?

    @ObservationIgnored private let core: String
    @ObservationIgnored private let agentId: String
    @ObservationIgnored private let connection: () -> Connection?
    @ObservationIgnored private let attached: () -> Bool
    @ObservationIgnored private let chat: () -> ChatRows
    @ObservationIgnored private let emit: () -> Void
    @ObservationIgnored private let onPending: (Bool) -> Void

    init(
        core: String, agentId: String, connection: @escaping () -> Connection?,
        attached: @escaping () -> Bool = { true }, chat: @escaping () -> ChatRows,
        emit: @escaping () -> Void, onPending: @escaping (Bool) -> Void = { _ in }
    ) {
        self.core = core
        self.agentId = agentId
        self.connection = connection
        self.attached = attached
        self.chat = chat
        self.emit = emit
        self.onPending = onPending
        let saved = OutboxStore.load(core: core, agentId: agentId)
        if !saved.isEmpty {
            entries = saved.map {
                let failed = $0.state == "failed"
                return Outgoing(
                    messageId: $0.messageId, text: $0.text, files: $0.files,
                    named: Named(paths: $0.paths, context: $0.context), at: $0.at,
                    state: failed ? .failed : .sending, problem: failed ? $0.problem : nil,
                    waiting: !failed, kept: true, steers: $0.steers)
            }
            for entry in saved { show(entry.files) }
        }
    }

    private func persistWaiting() {
        let saved = entries.filter { ($0.waiting || $0.kept) && $0.state != .sent }.map {
            OutboxStore.Saved(
                messageId: $0.messageId, text: $0.text, files: $0.files,
                paths: $0.named.paths, context: $0.named.context, at: $0.at, steers: $0.steers,
                state: $0.state == .failed ? "failed" : "waiting",
                problem: $0.state == .failed ? $0.problem : nil)
        }
        OutboxStore.save(core: core, agentId: agentId, entries: saved)
        onPending(waiting || sending)
    }

    /// Drops all in-memory and on-disk pending messages (used when a host is forgotten or unpaired).
    func clearAll() {
        epoch &+= 1
        entries.removeAll()
        holding.removeAll()
        OutboxStore.save(core: core, agentId: agentId, entries: [])
        onPending(false)
        emit()
    }

    /// Some message is still on its way.
    var sending: Bool { entries.contains { $0.state != .failed } }

    /// Some message waits for the chat to be attached.
    var waiting: Bool { entries.contains { $0.waiting } }

    /// `chat` with what this phone sent that the host does not show yet.
    func over(_ chat: ChatRows) -> ChatRows {
        let behind = notQueued(holding, chat.queued)
        if entries.isEmpty && behind.isEmpty { return chat }
        var view = chat
        if !entries.isEmpty { view.messages += entries.map(Self.ownRow) }
        let running = chat.running || sending
        view.setHead { head in
            head.running = running
            head.queued += behind.map(\.message)
        }
        return view
    }

    /// By message id.
    var unsent: [String: Unsent] {
        var unsent: [String: Unsent] = [:]
        for entry in entries where entry.state != .sent {
            unsent[entry.messageId] = Unsent(state: entry.state == .failed ? .failed : .sending, problem: entry.problem)
        }
        return unsent
    }

    func deliver(
        _ text: String, _ files: [Attachment], waiting: Bool = false, named: Named = Named(), steers: Bool = false
    ) {
        show(files)
        let entry = Outgoing(
            messageId: UUID().uuidString.lowercased(), text: text, files: files, named: named,
            at: Self.now, waiting: waiting, kept: waiting, steers: steers
        )
        entries.append(entry)
        if waiting { persistWaiting() }
        emit()
        if !waiting { push(entry) }
    }

    func retry(_ messageId: String) {
        guard let entry = entries.first(where: { $0.messageId == messageId }), entry.state == .failed else { return }
        update(messageId) {
            $0.state = .sending
            $0.problem = nil
        }
        push(entry)
    }

    /// Sends what was shown before the chat was attached, now that it is.
    func pushWaiting() {
        let waiting = entries.filter(\.waiting)
        guard !waiting.isEmpty else { return }
        let chat = chat()
        let shown = Set(chat.messages.compactMap(\.promptId))
        let queued = Set(chat.queued.map(\.id))
        let alreadyLanded: (String) -> Bool = { shown.contains($0) || queued.contains($0) }
        entries.removeAll { $0.waiting && alreadyLanded($0.messageId) }
        for at in entries.indices { entries[at].waiting = false }
        persistWaiting()
        let toSend = waiting.filter { !alreadyLanded($0.messageId) }
        guard !toSend.isEmpty else {
            emit()
            return
        }
        toSend.forEach(push)
    }

    func failWaiting(_ problem: String) {
        guard waiting else { return }
        for at in entries.indices where entries[at].waiting {
            entries[at].waiting = false
            entries[at].state = .failed
            entries[at].problem = problem
        }
        persistWaiting()
    }

    /// Sends the message behind the running turn, or into it when it `steers`, showing it queued at once. Throws when
    /// it did not go, once it is out of the queue.
    func queue(_ text: String, _ files: [Attachment], steers: Bool, named: Named) async throws {
        let id = UUID().uuidString.lowercased()
        show(files)
        let shown = files.map(Self.shownPath) + named.paths
        let context = named.context.map { SentContext(uri: $0.uri, title: $0.title) }
        holding.append(Holding(message: QueuedMessage(id: id, text: text, paths: shown, context: context)))
        emit()
        let connection = connection()
        let generation = epoch
        do {
            try await inOrder {
                guard self.epoch == generation else { return }
                guard let connection, connection.isOpen() else { throw ChatProblem.notConnected }
                let sent = try await self.put(connection, files) { _, _ in }
                guard self.epoch == generation else { return }
                let paths = Uploads.paths(sent) + named.paths
                if !steers { return try await self.prompt(connection, id, text, paths, named.context) }
                let ids = self.onHostQueue(self.chat().queued.map(\.id))
                let message = QueuedMessageParams(text: text, paths: paths, context: named.context)
                _ = try await connection.call(
                    PhoneCall.SteerQueued.self, QueuedParams(agentId: self.agentId, ids: ids, message: message))
            }
        } catch {
            holding.removeAll { $0.message.id == id }
            if epoch != generation { return }
            if !steers, connection?.isOpen() != true {
                entries.append(
                    Outgoing(
                        messageId: id, text: text, files: files, named: named,
                        at: Self.now, state: .sending, waiting: true, kept: true, steers: false))
                persistWaiting()
                emit()
                if let current = self.connection(), current !== connection, current.isOpen(), attached() {
                    pushWaiting()
                }
                return
            }
            throw error
        }
        guard epoch == generation else { return }
        if let held = holding.first(where: { $0.message.id == id }),
            steers || notQueued([held], chat().queued).isEmpty
        {
            holding.removeAll { $0.message.id == id }
        } else if let at = holding.firstIndex(where: { $0.message.id == id }) {
            holding[at].taken = true
        }
        emit()
    }

    /// Lets go of queued messages the host has taken, now that its own queue or turn says where they went.
    func dropTaken() {
        if holding.contains(where: \.taken) { holding.removeAll(where: \.taken) }
    }

    /// Of `ids`, the ones in the host's queue rather than still on their way to it.
    func onHostQueue(_ ids: [String]) -> [String] {
        let onTheirWay = Set(notQueued(holding, chat().queued).map(\.message.id))
        return ids.filter { !onTheirWay.contains($0) }
    }

    /// The host shows a message as the row that carries its id; one that met a running turn waits in the host's
    /// queue under it instead. Once a turn has ended, nothing the host took is still to come.
    func settle(turnEnded: Bool = false) {
        guard !entries.isEmpty else { return }
        let chat = chat()
        let shown = Set(chat.messages.compactMap(\.promptId))
        let queued = Set(chat.queued.map(\.id))
        let kept = entries.filter { entry in
            if shown.contains(entry.messageId) || queued.contains(entry.messageId) { return false }
            if turnEnded && entry.state == .sent { return false }
            return true
        }
        if kept.count != entries.count {
            let hadPersisted = entries.contains { $0.waiting || $0.kept }
            entries = kept
            if hadPersisted { persistWaiting() } else { onPending(waiting || sending) }
        }
    }

    /// Puts each of `files` the host does not have yet on it, answering them all with their paths or throwing why one
    /// did not get there.
    func put(_ connection: Connection?, _ files: [Attachment], mark: (String, (inout Attachment) -> Void) -> Void)
        async throws -> [Attachment]
    {
        let agentId = agentId
        let (sent, failure) = await Uploads.each(
            files,
            put: { attachment in
                guard let connection, connection.isOpen() else { throw ChatProblem.notConnected }
                let bytes = try await Task.detached { try Data(contentsOf: attachment.uri) }.value
                let path = try await connection.attachFile(
                    agentId: agentId, name: attachment.name, mime: attachment.mime, bytes: bytes)
                self.sentFiles[path] = attachment
                return path
            }, mark: mark)
        guard let sent else { throw ChatProblem(failure ?? String(localized: "a file did not reach the host")) }
        return sent
    }

    private func show(_ files: [Attachment]) {
        for file in files { sentFiles[Self.shownPath(file)] = file }
    }

    /// Sends a message; one that names agents, chats or issues goes as a call, since the phone's own request carries
    /// none.
    private func prompt(
        _ connection: Connection, _ messageId: String, _ text: String, _ paths: [String], _ context: [ChatContext]
    ) async throws {
        if context.isEmpty {
            try await connection.prompt(agentId: agentId, messageId: messageId, text: text, paths: paths)
            return
        }
        _ = try await connection.call(
            PhoneCall.PromptChat.self,
            PromptChatParams(agentId: agentId, messageId: messageId, text: text, paths: paths, context: context)
        )
    }

    private func inOrder(_ work: @escaping @MainActor () async throws -> Void) async throws {
        let before = line
        let next = Task { @MainActor in
            await before?.value
            return try await work()
        }
        line = Task { _ = try? await next.value }
        try await next.value
    }

    private func push(_ entry: Outgoing) {
        let connection = connection()
        let id = entry.messageId
        let generation = epoch
        Task {
            var steered = false
            do {
                try await inOrder {
                    guard self.epoch == generation else { return }
                    guard let connection, connection.isOpen() else { throw ChatProblem.notConnected }
                    let files = self.entries.first(where: { $0.messageId == id })?.files ?? entry.files
                    let sent = try await self.put(connection, files) { fileId, change in
                        self.update(id) { outgoing in
                            if let at = outgoing.files.firstIndex(where: { $0.id == fileId }) {
                                change(&outgoing.files[at])
                            }
                        }
                    }
                    guard self.epoch == generation else { return }
                    let paths = Uploads.paths(sent) + entry.named.paths
                    if entry.steers && self.chat().running {
                        steered = true
                        // Clear disk copy before putting a non-deduplicable steer on the wire.
                        self.update(id) { $0.kept = false }
                        self.persistWaiting()
                        let ids = self.onHostQueue(self.chat().queued.map(\.id))
                        let message = QueuedMessageParams(text: entry.text, paths: paths, context: entry.named.context)
                        _ = try await connection.call(
                            PhoneCall.SteerQueued.self,
                            QueuedParams(agentId: self.agentId, ids: ids, message: message))
                    } else {
                        try await self.prompt(connection, id, entry.text, paths, entry.named.context)
                    }
                }
                guard epoch == generation else { return }
                if steered {
                    entries.removeAll { $0.messageId == id }
                } else {
                    update(id) {
                        $0.state = .sent
                        $0.kept = false
                    }
                    settle()
                }
                persistWaiting()
                emit()
            } catch {
                guard epoch == generation else { return }
                if !entry.steers, connection?.isOpen() != true {
                    update(id) {
                        $0.waiting = true
                        $0.kept = true
                        $0.state = .sending
                        $0.problem = nil
                    }
                    persistWaiting()
                    if let current = self.connection(), current !== connection, current.isOpen(), attached() {
                        pushWaiting()
                    }
                    return
                }
                update(id) {
                    $0.state = .failed
                    $0.problem = problem(error)
                }
                persistWaiting()
            }
        }
    }

    private func update(_ messageId: String, _ change: (inout Outgoing) -> Void) {
        guard let at = entries.firstIndex(where: { $0.messageId == messageId }) else { return }
        change(&entries[at])
        emit()
    }

    /// The messages in `holding` the host's queue does not hold yet.
    private func notQueued(_ holding: [Holding], _ host: [QueuedMessage]) -> [Holding] {
        if holding.isEmpty { return [] }
        let queued = Set(host.map(\.id))
        return holding.filter { !queued.contains($0.message.id) }
    }

    private static func shownPath(_ file: Attachment) -> String { file.path ?? file.uri.path }

    private static var now: Int { Int(Date().timeIntervalSince1970 * 1000) }

    private static func ownRow(_ outgoing: Outgoing) -> ChatMessage {
        let text = outgoing.text.trimmingCharacters(in: .whitespacesAndNewlines)
        let context = outgoing.named.context.map { SentContext(uri: $0.uri, title: $0.title) }
        return ChatMessage(
            id: outgoing.messageId, role: .user, promptId: outgoing.messageId,
            parts: text.isEmpty ? [] : [.text(ChatPart.Text(id: "\(outgoing.messageId)/text", text: outgoing.text))],
            attachments: outgoing.files.map(shownPath) + outgoing.named.paths,
            context: context.isEmpty ? nil : context,
            sentAt: outgoing.at
        )
    }
}
