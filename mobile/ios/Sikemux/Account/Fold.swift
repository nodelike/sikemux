import Foundation
import Observation

/// Whether the signed-in account may go on to its screens. A new account may first be asked whether the person already
/// has one, and a fold this phone started is confirmed here. The phone joins the account only once the gate is open.
@MainActor @Observable
final class Fold {
    static let shared = Fold()

    /// A fold this phone started, waiting for the person's other account to confirm it. Kept in memory only.
    struct Waiting: Hashable {
        let id: String
        let secret: String
        let expiresAt: Date
        let why: FoldWhy
        let words: String
    }

    enum Gate: Equatable {
        case checking
        case open
        case caught(FoldCatch)
        case confirm(Waiting)
    }

    private(set) var gate: Gate = .checking
    private(set) var waiting: Waiting?

    @ObservationIgnored private var userId: String?
    @ObservationIgnored private var deciding: Task<Void, Never>?

    private static let signedInHere = "sikemux.signed-in-here"
    private static func settled(_ userId: String) -> String { "sikemux.one-account.\(userId)" }

    private init() {}

    /// Decides the gate for a new sign-in; nil on signing out.
    func follow(_ userId: String?) {
        deciding?.cancel()
        self.userId = userId
        gate = .checking
        guard let userId else { return }
        if let waiting, waiting.expiresAt > .now {
            gate = .confirm(waiting)
            return
        }
        waiting = nil
        if UserDefaults.standard.bool(forKey: Self.settled(userId)) { return open() }
        ask(userId)
    }

    /// Asks the server again whether this account may stand, after it refused to add the phone.
    func askAgain() {
        guard let userId else { return }
        deciding?.cancel()
        gate = .checking
        ask(userId)
    }

    /// Lets the account in, and remembers it so it is not asked again.
    func open() {
        if let userId { UserDefaults.standard.set(true, forKey: Self.settled(userId)) }
        UserDefaults.standard.set(true, forKey: Self.signedInHere)
        gate = .open
        AccountSession.shared.admit()
    }

    func hold(_ intent: FoldIntent, why: FoldWhy) {
        waiting = Waiting(
            id: intent.id, secret: intent.secret, expiresAt: Format.date(iso: intent.expiresAt) ?? .now, why: why,
            words: intent.waitingWords)
    }

    func drop() {
        waiting = nil
    }

    private func ask(_ userId: String) {
        let before = UserDefaults.standard.bool(forKey: Self.signedInHere)
        deciding = Task {
            do {
                let caught = try await AccountAPI.send(
                    "POST", "/v1/account/fold/catch", body: FoldCatchRequest(hint: before ? .browser : nil),
                    as: FoldCatch.self)
                guard !Task.isCancelled, self.userId == userId else { return }
                if caught.ask { gate = .caught(caught) } else { open() }
            } catch {
                guard !Task.isCancelled, self.userId == userId else { return }
                gate = .open
                AccountSession.shared.admit()
            }
        }
    }
}

@MainActor
extension AccountCalls {
    static func startFold(why: FoldWhy) async throws -> FoldIntent {
        try await AccountAPI.send("POST", "/v1/account/folds", body: FoldStart(why: why), as: FoldIntent.self)
    }

    static func previewFold(_ waiting: Fold.Waiting) async throws -> FoldPreview {
        try await AccountAPI.send(
            "POST", "/v1/account/folds/\(waiting.id)/preview", body: FoldSecret(secret: waiting.secret),
            as: FoldPreview.self)
    }

    static func confirmFold(_ waiting: Fold.Waiting) async throws {
        try await AccountAPI.send("POST", "/v1/account/folds/\(waiting.id)/confirm", body: FoldSecret(secret: waiting.secret))
    }

    static func keepNewAccount() async throws {
        try await AccountAPI.send("POST", "/v1/account/fold/keep-new")
    }
}
