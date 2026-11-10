import Foundation
import Observation
import SikemuxCore

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
    /// Messages reach the host in the order they were sent, however long their files take.
    @ObservationIgnored private var line: Task<Void, Never>?

    @ObservationIgnored private let agentId: String
    @ObservationIgnored private let connection: () -> Connection?
    @ObservationIgnored private let chat: () -> ChatRows
    @ObservationIgnored private let emit: () -> Void

    init(agentId: String, connection: @escaping () -> Connection?, chat: @escaping () -> ChatRows, emit: @escaping () -> Void) {
        self.agentId = agentId
        self.connection = connection
        self.chat = chat
        self.emit = emit
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
        if !entries.isEmpty {
            view.messages += entries.map(Self.ownRow)
            view.running = chat.running || sending
        }
        if !behind.isEmpty { view.queued += behind.map(\.message) }
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

    func deliver(_ text: String, _ files: [Attachment], waiting: Bool = false, named: Named = Named()) {
        show(files)
        let entry = Outgoing(
            messageId: UUID().uuidString.lowercased(), text: text, files: files, named: named,
            at: Self.now, waiting: waiting
        )
        entries.append(entry)
        emit()
        if !waiting { push(entry) }
    }

    func retry(_ messageId: String) {
        guard let entry = entries.first(where: { $0.messageId == messageId }), entry.state == .failed else { return }
        update(messageId) { $0.state = .sending; $0.problem = nil }
        push(entry)
    }

    /// Sends what was shown before the chat was attached, now that it is.
    func pushWaiting() {
        let waiting = entries.filter(\.waiting)
        guard !waiting.isEmpty else { return }
        for at in entries.indices { entries[at].waiting = false }
        waiting.forEach(push)
    }

    func failWaiting(_ problem: String) {
        guard waiting else { return }
        for at in entries.indices where entries[at].waiting {
            entries[at].waiting = false
            entries[at].state = .failed
            entries[at].problem = problem
        }
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
        do {
            try await inOrder {
                guard let connection, connection.isOpen() else { throw OutboxError.closed }
                let sent = try await self.put(connection, files) { _, _ in }
                let paths = Uploads.paths(sent) + named.paths
                if !steers { return try await self.prompt(connection, id, text, paths, named.context) }
                let ids = self.onHostQueue(self.chat().queued.map(\.id))
                let message = QueuedMessageParams(text: text, paths: paths, context: named.context)
                _ = try await connection.call(PhoneCall.SteerQueued.self, QueuedParams(agentId: self.agentId, ids: ids, message: message))
            }
        } catch {
            holding.removeAll { $0.message.id == id }
            throw error
        }
        if let held = holding.first(where: { $0.message.id == id }),
           steers || notQueued([held], chat().queued).isEmpty {
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
        guard entries.contains(where: { $0.state == .sent }) else { return }
        if turnEnded {
            entries.removeAll { $0.state == .sent }
            return
        }
        let chat = chat()
        let shown = Set(chat.messages.compactMap(\.promptId))
        let queued = Set(chat.queued.map(\.id))
        let kept = entries.filter { $0.state != .sent || (!shown.contains($0.messageId) && !queued.contains($0.messageId)) }
        if kept.count != entries.count { entries = kept }
    }

    /// Puts each of `files` the host does not have yet on it, answering them all with their paths or throwing why one
    /// did not get there.
    func put(_ connection: Connection?, _ files: [Attachment], mark: (String, (inout Attachment) -> Void) -> Void) async throws -> [Attachment] {
        let agentId = agentId
        let (sent, failure) = await Uploads.each(files, put: { attachment in
            guard let connection, connection.isOpen() else { throw OutboxError.closed }
            let bytes = try await Task.detached { try Data(contentsOf: attachment.uri) }.value
            let path = try await connection.attachFile(agentId: agentId, name: attachment.name, mime: attachment.mime, bytes: bytes)
            self.sentFiles[path] = attachment
            return path
        }, mark: mark)
        guard let sent else { throw OutboxError.failed(failure ?? "a file did not reach the host") }
        return sent
    }

    private func show(_ files: [Attachment]) {
        for file in files { sentFiles[Self.shownPath(file)] = file }
    }

    /// Sends a message; one that names agents, chats or issues goes as a call, since the phone's own request carries
    /// none.
    private func prompt(_ connection: Connection, _ messageId: String, _ text: String, _ paths: [String], _ context: [ChatContext]) async throws {
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
        Task {
            do {
                try await inOrder {
                    guard let connection, connection.isOpen() else { throw OutboxError.closed }
                    let files = self.entries.first(where: { $0.messageId == id })?.files ?? entry.files
                    let sent = try await self.put(connection, files) { fileId, change in
                        self.update(id) { outgoing in
                            if let at = outgoing.files.firstIndex(where: { $0.id == fileId }) { change(&outgoing.files[at]) }
                        }
                    }
                    try await self.prompt(connection, id, entry.text, Uploads.paths(sent) + entry.named.paths, entry.named.context)
                }
                update(id) { $0.state = .sent }
                settle()
                emit()
            } catch {
                update(id) { $0.state = .failed; $0.problem = problem(error) }
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

enum OutboxError: LocalizedError {
    case closed
    case failed(String)

    var errorDescription: String? {
        switch self {
        case .closed: "the host is not connected"
        case .failed(let why): why
        }
    }
}
