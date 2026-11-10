import Foundation

/// This phone's connection to the account at `/v1/live`: hears the account's changes as they happen, and learns when
/// the phone was taken off it. It reconnects with backoff while running.
@MainActor
final class LiveAccount {
    enum Gone: String {
        case removed, signedOut = "signed_out", accountDeleted = "account_deleted"
    }

    struct Handlers {
        /// A host joined, changed or left the account: the list of hosts is stale.
        var hostsChanged: () -> Void
        var stashChanged: () -> Void
        var deviceAdded: (String) -> Void
        /// The server took the hello, so the account is reachable again.
        var ready: () -> Void
        /// Called once; the connection stays closed after it.
        var gone: (Gone) -> Void
    }

    private struct Event: Decodable {
        let id: Int
        let type: String
        let key: String?
        let role: String?
        let reason: String?
        let at: String?
    }

    private struct Message: Decodable {
        let type: String
        let nonce: String?
        let heartbeatMs: Double?
        let events: [Event]?
        let reconnectAfterMs: Double?
        let reason: String?
    }

    private static let cursorItem = "sikemux.account-event"
    /// How long after a device joins the account the person still hears of it; a phone back from days offline is not
    /// told of every one.
    private static let newsFor: TimeInterval = 10 * 60

    private enum CloseCode {
        static let revoked = 4403
        static let overLimit = 4429
        static let replaced = 4409
    }

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

    private func send(_ task: URLSessionWebSocketTask, _ message: [String: Any]) {
        guard let data = try? JSONSerialization.data(withJSONObject: message) else { return }
        task.send(.string(String(decoding: data, as: UTF8.self))) { _ in }
    }

    private func handle(_ task: URLSessionWebSocketTask, _ text: String) async {
        guard let message = try? JSONDecoder().decode(Message.self, from: Data(text.utf8)) else { return }
        switch message.type {
        case "challenge":
            guard let nonce = message.nonce else { return }
            do {
                let me = try Identity.shared.deviceIdentity()
                let token = try await Account.shared.token()
                ownKey = me.id()
                guard socket === task else { return }
                send(task, [
                    "type": "hello", "role": "client", "key": ownKey, "signature": try me.signLive(nonce: nonce),
                    "token": token, "app": ["platform": "ios", "version": Config.version],
                ])
            } catch {
                if !Account.shared.isSignedIn { stop() }
            }
        case "ready":
            attempt = 0
            if let ms = message.heartbeatMs { heartbeat = ms / 1000 }
            heard()
            handlers.ready()
        case "events":
            let (ack, gone) = read(message.events ?? [])
            if socket === task, ack > 0 { send(task, ["type": "ack", "id": ack]) }
            if let gone { end(gone) }
        case "reset":
            handlers.hostsChanged()
            handlers.stashChanged()
        case "ping":
            send(task, ["type": "pong"])
        case "bye":
            byeAfter = (message.reconnectAfterMs ?? 0) / 1000
        case "revoked":
            end(Gone(rawValue: message.reason ?? "") ?? .removed)
        default:
            break
        }
    }

    private func read(_ events: [Event]) -> (ack: Int, gone: Gone?) {
        let mark = (try? Keychain.read(Self.cursorItem)).flatMap { $0 }.flatMap(Int.init) ?? 0
        var newest = 0
        var handled = mark
        var hosts = false
        var stash = false
        var added: [String] = []
        var gone: Gone?
        for event in events.sorted(by: { $0.id < $1.id }) {
            newest = max(newest, event.id)
            guard event.id > mark else { continue }
            handled = event.id
            if event.type == "account.deleted" {
                gone = .accountDeleted
            } else if event.type == "device.revoked", event.key == ownKey {
                gone = gone ?? Gone(rawValue: event.reason ?? "") ?? .removed
            } else if event.type.hasPrefix("stash.") {
                stash = true
            } else {
                if event.type == "device.added", let key = event.key, key != ownKey,
                   let at = Format.date(iso: event.at), Date().timeIntervalSince(at) < Self.newsFor
                {
                    added.append(key)
                }
                if event.role == "host" { hosts = true }
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

    private func end(_ reason: Gone) {
        guard !ended else { return }
        ended = true
        stop()
        handlers.gone(reason)
    }

    private func closed(_ code: Int) {
        silence?.cancel()
        guard running else { return }
        if code == CloseCode.revoked { return end(.removed) }
        if code == CloseCode.replaced {
            running = false
            return
        }
        var wait: TimeInterval
        if let byeAfter {
            wait = byeAfter + Double.random(in: 0..<1)
            self.byeAfter = nil
        } else {
            wait = AccountSession.backoff(attempt)
            if code == CloseCode.overLimit { wait = max(wait, 30) }
            attempt += 1
        }
        retrying = Task { [weak self] in
            try? await Task.sleep(for: .seconds(wait))
            guard !Task.isCancelled, let self, self.running else { return }
            self.open()
        }
    }
}
