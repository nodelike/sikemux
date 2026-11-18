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
    /// The account whose sign-in couldn't ask "Already on Sikemux?", how often the server has since answered with an error, and when the first came.
    @ObservationIgnored private var unasked: (userId: String, failures: Int, since: Date)?
    @ObservationIgnored private var deciding: Task<Void, Never>?

    private static let signedInHere = "sikemux.signed-in-here"
    private static func settled(_ userId: String) -> String { "sikemux.one-account.\(userId)" }
    private static let giveUpAfter = 3
    private static let giveUpAfterSeconds: TimeInterval = 60

    /// Thrown when the person is asked before the phone may join.
    struct Asking: Error {}

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

    func hold(_ intent: FoldIntent) {
        waiting = Waiting(
            id: intent.id, secret: intent.secret, expiresAt: Format.date(iso: intent.expiresAt) ?? .now,
            words: intent.waitingWords)
    }

    func drop() {
        waiting = nil
    }

    /// Before the phone joins an account whose sign-in couldn't ask "Already on Sikemux?", asks it now, since joining
    /// ends the offer. Throws while the server can't be reached, so joining waits and tries again.
    func askBeforeJoining(_ userId: String) async throws {
        guard unasked?.userId == userId else { return }
        if await onAccount() {
            guard self.userId == userId else { throw CancellationError() }
            unasked = nil
            return open()
        }
        do {
            let caught = try await askCatch()
            guard self.userId == userId else { throw CancellationError() }
            unasked = nil
            if caught.ask {
                gate = .caught(caught)
                throw Asking()
            }
            open()
        } catch let problem as ApiProblem {
            guard let status = problem.status, unasked?.userId == userId else { throw problem }
            unasked!.failures += 1
            if unasked!.failures == 1 { unasked!.since = .now }
            let lasting = unasked!.failures >= Self.giveUpAfter && -unasked!.since.timeIntervalSinceNow >= Self.giveUpAfterSeconds
            if (problem.unreachable || status == 429) && !lasting { throw problem }
            unasked = nil
        }
    }

    /// Whether the account already holds this phone, so the catch has nothing left to offer.
    private func onAccount() async -> Bool {
        guard let key = try? Identity.shared.deviceIdentity().id() else { return false }
        return (try? await AccountCalls.device(key)) != nil
    }

    private func askCatch() async throws -> FoldCatch {
        let before = UserDefaults.standard.bool(forKey: Self.signedInHere)
        return try await AccountAPI.send(
            "POST", "/v1/account/fold/catch", body: FoldCatchRequest(hint: before ? .browser : nil), as: FoldCatch.self)
    }

    private func ask(_ userId: String) {
        deciding = Task {
            do {
                let caught = try await askCatch()
                guard !Task.isCancelled, self.userId == userId else { return }
                if caught.ask { gate = .caught(caught) } else { open() }
            } catch {
                guard !Task.isCancelled, self.userId == userId else { return }
                unasked = (userId, 0, .now)
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
