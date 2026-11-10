import Foundation
import Observation

/// One shared chat, watched read-only through the accounts server. It holds the rows the host's snapshot sends and
/// makes the changes that follow, and asks the host, sealed, for the turns before them and for what a tool call did.
@MainActor @Observable
final class ShareSession {
    enum Status {
        /// Reaching the server, or waiting for the chat's first snapshot.
        case connecting
        case live
        /// The chat's agent is not running on the sharer's computer now.
        case missing
        case ended
        case notFound
        case signedOut
        /// The link's key does not read what the host sent.
        case unreadable
    }

    private(set) var status = Status.connecting
    private(set) var endedBecause: ShareServerMessage.EndReason?
    private(set) var hostOnline = false
    private(set) var expiresAt: Date?
    private(set) var chat: SharedChat?
    private(set) var rows = ChatRows.empty
    private(set) var earlier = Feed.Earlier.idle
    /// Counts each change to the rows, so the transcript knows to look at `touched`.
    private(set) var drawn = 0
    @ObservationIgnored private(set) var touched = Touched()

    /// A question the host has not answered in this long gets no answer.
    private static let answerWait: Duration = .seconds(30)

    @ObservationIgnored private let link: SharedChatLink
    @ObservationIgnored private var socket: URLSessionWebSocketTask?
    @ObservationIgnored private var stopped = true
    @ObservationIgnored private var failures = 0
    @ObservationIgnored private var retrying: Task<Void, Never>?
    @ObservationIgnored private var renewing: Task<Void, Never>?
    /// The last frame taken, so a reconnect asks only for what came after it.
    @ObservationIgnored private var seq: Int?
    /// The run of the chat's agent the rows come from, which earlier pages are asked of.
    @ObservationIgnored private var feed: String?
    @ObservationIgnored private var chatSeq = 0
    @ObservationIgnored private var asked = 0
    @ObservationIgnored private var answers: [Int: CheckedContinuation<ShareReply?, Never>] = [:]

    init(link: SharedChatLink) {
        self.link = link
    }

    var hasEarlier: Bool { feed != nil && rows.olderBefore != nil }

    /// What a shared chat is called: the app's name for it, else its first message, else its agent's.
    var title: String {
        if let title = chat?.title, !title.isEmpty { return title }
        if let title = rows.head.title, !title.isEmpty { return title }
        let first = rows.messages.first { $0.role == .user }
        let words = (first?.parts.compactMap { part -> String? in
            if case .text(let text) = part { return text.text }
            return nil
        }.first ?? "").split(whereSeparator: \.isWhitespace).joined(separator: " ")
        if !words.isEmpty { return words.count > 80 ? "\(words.prefix(79))…" : words }
        return "\(Catalog.label(chat?.provider ?? "agent")) chat"
    }

    func takeTouched() -> Touched {
        defer { touched = Touched() }
        return touched
    }

    func start() {
        guard stopped else { return }
        stopped = false
        open()
    }

    func stop() {
        stopped = true
        retrying?.cancel()
        renewing?.cancel()
        socket?.cancel(with: .normalClosure, reason: nil)
        socket = nil
        settleAnswers()
    }

    /// Fetches the turns before the first message shown, unless that is already under way.
    func loadEarlier() {
        guard let feed, let before = rows.olderBefore, earlier != .loading else { return }
        earlier = .loading
        Task {
            let answer = await ask(.page(.init(id: 0, feed: feed, before: before)))
            guard self.feed == feed, rows.olderBefore == before else { return }
            if let page = answer?.page {
                rows.addPage(page)
                touched.all = true
                drawn += 1
                earlier = .idle
            } else {
                earlier = .failed
            }
        }
    }

    /// What a tool call changed and printed, or an error when the host will not or cannot say.
    func toolDetail(_ toolCallId: String) async throws -> ToolDetail {
        guard let detail = await ask(.toolDetail(.init(id: 0, toolCallId: toolCallId)))?.toolDetail else {
            throw SharerSilent()
        }
        return detail
    }

    // MARK: The socket

    private func open() {
        var url = URLComponents(url: Config.apiUrl.appending(path: "v1/share"), resolvingAgainstBaseURL: false)!
        url.scheme = url.scheme == "https" ? "wss" : "ws"
        let socket = URLSession.shared.webSocketTask(with: url.url!)
        self.socket = socket
        socket.resume()
        Task { await read(socket) }
    }

    private func read(_ socket: URLSessionWebSocketTask) async {
        while self.socket === socket {
            do {
                guard case .string(let text) = try await socket.receive() else { continue }
                let message: ShareServerMessage
                do {
                    message = try JSONDecoder().decode(ShareServerMessage.self, from: Data(text.utf8))
                } catch {
                    print("sikemux: the share server sent a message this app cannot read: \(error)")
                    continue
                }
                await handle(socket, message)
            } catch {
                guard self.socket === socket else { return }
                self.socket = nil
                closed(socket.closeCode.rawValue)
                return
            }
        }
    }

    private func send(_ socket: URLSessionWebSocketTask, _ message: ShareClientMessage) {
        guard self.socket === socket else { return }
        do {
            let text = String(decoding: try JSONEncoder().encode(message), as: UTF8.self)
            socket.send(.string(text)) { failed in
                if let failed { print("sikemux: could not send to the share: \(failed)") }
            }
        } catch {
            print("sikemux: could not write a message to the share: \(error)")
        }
    }

    private func closed(_ code: Int) {
        renewing?.cancel()
        settleAnswers()
        guard !stopped else { return }
        let known = ShareCloseCode(rawValue: code)
        if known == .notFound { return finish(.notFound) }
        if known == .ended { return finish(.ended) }
        hostOnline = false
        failures += 1
        let ceiling = min(60, pow(2, Double(failures - 1)))
        var wait = ceiling / 2 + ceiling / 2 * Double.random(in: 0...1)
        if known == .overLimit { wait = max(30, wait) }
        retrying = Task {
            try? await Task.sleep(for: .seconds(wait))
            if !Task.isCancelled, !stopped { open() }
        }
    }

    private func finish(_ status: Status) {
        self.status = status
        stop()
    }

    private func handle(_ socket: URLSessionWebSocketTask, _ message: ShareServerMessage) async {
        switch message {
        case .challenge:
            let token: String
            do { token = try await Account.shared.token() } catch { return finish(.signedOut) }
            send(socket, .hello(token: token, share: link.id, proof: link.viewerProof, since: seq))
        case .ready(let ready):
            failures = 0
            hostOnline = ready.hostOnline ?? false
            expiresAt = Format.date(iso: ready.expiresAt)
            if let renew = Format.date(iso: ready.authExpiresAt) { renewAuth(socket, before: renew) }
        case .frames(let frames):
            for frame in frames {
                guard let plain = link.open(frame.sealed) else { return finish(.unreadable) }
                seq = frame.seq
                take(plain)
            }
        case .answer(let sealed):
            guard let plain = link.open(sealed) else { return }
            do {
                let answer = try JSONDecoder().decode(ShareReply.self, from: Data(plain.utf8))
                answers.removeValue(forKey: answer.id)?.resume(returning: answer.error == nil ? answer : nil)
            } catch {
                print("sikemux: the sharer's computer sent an answer this app cannot read: \(error)")
            }
        case .host(let online):
            hostOnline = online
        case .ended(let reason):
            endedBecause = reason
            finish(.ended)
        case .ping:
            send(socket, .pong)
        case .other:
            break
        }
    }

    /// Sends a fresh sign-in before the server would close the connection for it.
    private func renewAuth(_ socket: URLSessionWebSocketTask, before expiry: Date) {
        renewing?.cancel()
        renewing = Task {
            try? await Task.sleep(for: .seconds(max(0, expiry.timeIntervalSinceNow - 15)))
            guard !Task.isCancelled else { return }
            do {
                send(socket, .auth(token: try await Account.shared.token(fresh: true)))
            } catch {
                print("sikemux: could not renew the sign-in for a shared chat: \(error)")
            }
        }
    }

    /// One unsealed frame: a snapshot starts the chat over, the rest change it.
    private func take(_ plain: String) {
        let frame: ShareFrame
        do {
            frame = try JSONDecoder().decode(ShareFrame.self, from: Data(plain.utf8))
        } catch {
            print("sikemux: the sharer's computer sent a frame this app cannot read: \(error)")
            return
        }
        switch frame {
        case .snapshot(let snapshot):
            chat = snapshot.chat
            Catalog.set("share", snapshot.chat.agents ?? [])
            if case .live(let live) = snapshot.attachment {
                rows = ChatRows(view: live.view)
                feed = live.mark.feed
                chatSeq = live.mark.seq
                earlier = .idle
                status = .live
            } else {
                status = .missing
                feed = nil
                if rows.messages.isEmpty { rows = .empty } else { rows.setHead { $0.running = false } }
            }
            touched.all = true
            drawn += 1
        case .event(let event):
            guard case .chat(let changes) = event.event, changes.agentId == chat?.agentId, changes.seq > chatSeq else { return }
            chatSeq = changes.seq
            rows.apply(changes.changes, touched: &touched)
            drawn += 1
        case .unknown:
            break
        }
    }

    /// Asks the host something, sealed. Answers nil when the host is away or will not say.
    private func ask(_ question: ShareAsk) async -> ShareReply? {
        guard let socket, hostOnline else { return nil }
        asked += 1
        let id = asked
        let numbered: ShareAsk
        switch question {
        case .page(var page): page.id = id; numbered = .page(page)
        case .toolDetail(var detail): detail.id = id; numbered = .toolDetail(detail)
        case .file(var file): file.id = id; numbered = .file(file)
        case .unknown: return nil
        }
        let sealed: String
        do {
            guard let made = link.seal(String(decoding: try JSONEncoder().encode(numbered), as: UTF8.self)) else { return nil }
            sealed = made
        } catch {
            print("sikemux: could not write a question for the sharer: \(error)")
            return nil
        }
        let timeout = Task {
            try? await Task.sleep(for: Self.answerWait)
            if !Task.isCancelled { answers.removeValue(forKey: id)?.resume(returning: nil) }
        }
        defer { timeout.cancel() }
        return await withCheckedContinuation { waiting in
            answers[id] = waiting
            send(socket, .ask(sealed: sealed))
        }
    }

    private func settleAnswers() {
        let waiting = answers
        answers = [:]
        for answer in waiting.values { answer.resume(returning: nil) }
    }
}

/// The sharer's computer is away or would not say.
private struct SharerSilent: LocalizedError {
    var errorDescription: String? { "The sharer's computer did not send this." }
}
