import Foundation

/// This phone's connection to the account at `/v1/live`: hears the account's changes as they happen, and learns when
/// the phone was taken off it. It reconnects with backoff while running.
@MainActor
final class LiveAccount {
    struct Handlers {
        /// A host joined, changed or left the account: the list of hosts is stale.
        var hostsChanged: () -> Void
        var stashChanged: () -> Void
        var deviceAdded: (String) -> Void
        /// The server took the hello, so the account is reachable again.
        var ready: () -> Void
        /// Called once; the connection stays closed after it.
        var gone: (RevokeReason) -> Void
    }

    private static let cursorItem = "sikemux.account-event"
    /// How long after a device joins the account the person still hears of it; a phone back from days offline is not
    /// told of every one.
    private static let newsFor: TimeInterval = 10 * 60

    private let handlers: Handlers
    private var socket: URLSessionWebSocketTask?
    private var running = false
    private var ended = false
    private var attempt = 0
    private var heartbeat: TimeInterval = 25
    private var byeAfter: TimeInterval?
    private var retrying: Task<Void, Never>?
    private var silence: Task<Void, Never>?
    private var ownKey = ""

    init(handlers: Handlers) {
        self.handlers = handlers
    }

    static func forgetCursor() {
        Keychain.delete(cursorItem)
    }

    func start() {
        guard !running, !ended else { return }
        running = true
        attempt = 0
        open()
    }

    /// Tries again now rather than when the backoff says, as when the network comes back.
    func nudge() {
        guard running, socket == nil else { return }
        attempt = 0
        open()
    }

    func stop() {
        running = false
        retrying?.cancel()
        silence?.cancel()
        let closing = socket
        socket = nil
        closing?.cancel(with: .normalClosure, reason: nil)
    }

    private func open() {
        retrying?.cancel()
        var parts = URLComponents(url: Config.apiUrl.appending(path: "v1/live"), resolvingAgainstBaseURL: false)!
        parts.scheme = parts.scheme == "https" ? "wss" : "ws"
        let task = URLSession.shared.webSocketTask(with: parts.url!)
        socket = task
        task.resume()
        heard()
        receive(task)
    }

    private func receive(_ task: URLSessionWebSocketTask) {
        task.receive { [weak self] result in
            Task { @MainActor in
                guard let self, self.socket === task else { return }
                switch result {
                case .success(.string(let text)):
                    self.heard()
                    await self.handle(task, text)
                    if self.socket === task { self.receive(task) }
                case .success:
                    self.receive(task)
                case .failure:
                    self.socket = nil
                    self.closed(task.closeCode.rawValue)
                }
            }
        }
    }

    /// Something arrived, so the connection is alive; silence for two heartbeats means it is not.
    private func heard() {
        silence?.cancel()
        let task = socket
        let wait = heartbeat * 2
        silence = Task { [weak self] in
            try? await Task.sleep(for: .seconds(wait))
            guard !Task.isCancelled, let self, let task, self.socket === task else { return }
            self.socket = nil
            task.cancel(with: .normalClosure, reason: nil)
            self.closed(0)
        }
    }

    private func send(_ task: URLSessionWebSocketTask, _ message: some Encodable) {
        do {
            let data = try JSONEncoder().encode(message)
            task.send(.string(String(decoding: data, as: UTF8.self))) { error in
                if let error { print("sikemux: the account connection could not send: \(error)") }
            }
        } catch {
            print("sikemux: the account connection could not write a message: \(error)")
        }
    }

    private func handle(_ task: URLSessionWebSocketTask, _ text: String) async {
        let message: LiveServerMessage
        do {
            message = try JSONDecoder().decode(LiveServerMessage.self, from: Data(text.utf8))
        } catch {
            print("sikemux: the account connection could not read a message: \(error)")
            return
        }
        switch message {
        case .challenge(let challenge):
            do {
                let me = try Identity.shared.deviceIdentity()
                let token = try await Account.shared.token()
                ownKey = me.id()
                guard socket === task else { return }
                send(
                    task,
                    LiveHello(
                        role: .client, key: ownKey, signature: try me.signLive(nonce: challenge.nonce), token: token,
                        app: LiveApp(platform: .ios, version: Config.version)
                    ))
            } catch {
                print("sikemux: could not answer the account connection's challenge: \(error)")
                if !Account.shared.isSignedIn { stop() }
            }
        case .ready(let ready):
            attempt = 0
            heartbeat = TimeInterval(ready.heartbeatMs) / 1000
            heard()
            handlers.ready()
        case .events(let frame):
            let (ack, gone) = read(frame.events)
            if socket === task, ack > 0 { send(task, LiveAck(id: ack)) }
            if let gone { end(gone) }
        case .reset:
            handlers.hostsChanged()
            handlers.stashChanged()
        case .ping:
            send(task, LivePong())
        case .bye(let bye):
            byeAfter = TimeInterval(bye.reconnectAfterMs) / 1000
        case .revoked(let revoked):
            end(revoked.reason)
        case .pushed, .unknown:
            break
        }
    }

    private func read(_ events: [AccountEvent]) -> (ack: EventId, gone: RevokeReason?) {
        let mark = (try? Keychain.read(Self.cursorItem)).flatMap { $0 }.flatMap(Int.init) ?? 0
        var newest = 0
        var handled = mark
        var hosts = false
        var stash = false
        var added: [String] = []
        var gone: RevokeReason?
        for event in events.sorted(by: { $0.id < $1.id }) {
            newest = max(newest, event.id)
            guard event.id > mark else { continue }
            handled = event.id
            switch event.type {
            case .accountDeleted:
                gone = .accountDeleted
            case .deviceRevoked where event.key == ownKey:
                gone = gone ?? event.reason ?? .removed
            case .stashAdded, .stashChanged, .stashRemoved, .stashSeen:
                stash = true
            default:
                if event.type == .deviceAdded, let key = event.key, key != ownKey,
                    let at = Format.date(iso: event.at), Date().timeIntervalSince(at) < Self.newsFor
                {
                    added.append(key)
                }
                if event.role == .host { hosts = true }
            }
        }
        if handled > mark {
            do {
                try Keychain.write(Self.cursorItem, String(handled))
            } catch {
                print("sikemux: could not keep the account's event cursor: \(error)")
            }
        }
        if hosts { handlers.hostsChanged() }
        if stash { handlers.stashChanged() }
        added.forEach(handlers.deviceAdded)
        return (newest, gone)
    }

    private func end(_ reason: RevokeReason) {
        guard !ended else { return }
        ended = true
        stop()
        handlers.gone(reason)
    }

    private func closed(_ code: Int) {
        silence?.cancel()
        guard running else { return }
        let code = LiveCloseCode(rawValue: code)
        if code == .revoked { return end(.removed) }
        if code == .replaced {
            running = false
            return
        }
        var wait: TimeInterval
        if let byeAfter {
            wait = byeAfter + Double.random(in: 0..<1)
            self.byeAfter = nil
        } else {
            wait = AccountSession.backoff(attempt)
            if code == .overLimit { wait = max(wait, 30) }
            attempt += 1
        }
        retrying = Task { [weak self] in
            try? await Task.sleep(for: .seconds(wait))
            guard !Task.isCancelled, let self, self.running else { return }
            self.open()
        }
    }
}
