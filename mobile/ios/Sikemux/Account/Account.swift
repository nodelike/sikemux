import Foundation
import OSLog
import Observation
import UIKit

private let log = Logger(subsystem: "com.nodelike.sikemux", category: "account")

/// Who this phone is signed in as: Clerk's session, a fresh token for the accounts server, and the person's name,
/// email and picture.
@MainActor @Observable
final class Account {
    enum State: Equatable {
        case loading
        case signedOut
        case signedIn(userId: String)
    }

    /// What the phone keeps of the sign-in, so it opens signed in without the network.
    private struct Saved: Codable {
        let session: String
        let user: Clerk.User
    }

    static let shared = Account()
    private static let savedItem = "sikemux.account"

    private(set) var state: State = .loading
    private(set) var user: Clerk.User?
    @ObservationIgnored private var session: String?
    @ObservationIgnored private var cached: (jwt: String, expires: Date)?
    @ObservationIgnored private var fetching: Task<String, Error>?

    var isSignedIn: Bool {
        if case .signedIn = state { return true }
        return false
    }

    var userId: String? { user?.id }

    var sessionId: String? { session }

    func load() async {
        let saved: Saved?
        do {
            saved =
                try Clerk.hasClient()
                ? Keychain.app.read(Self.savedItem).flatMap {
                    try? JSONDecoder().decode(Saved.self, from: Data($0.utf8))
                }
                : nil
        } catch let refused as KeychainError where refused.status == errSecInteractionNotAllowed && !Self.inFront {
            log.info("the phone is locked, so the sign-in is read once it is unlocked")
            await Self.unlocked()
            guard !Task.isCancelled else { return }
            return await load()
        } catch {
            log.error("the Keychain refused the sign-in, so the phone starts signed out: \(error, privacy: .public)")
            state = .signedOut
            return
        }
        guard let saved else {
            state = .signedOut
            return
        }
        apply(session: saved.session, user: saved.user)
        Task { await refresh() }
    }

    /// In front with the phone unlocked, where a Keychain that refuses is not waiting for an unlock.
    private static var inFront: Bool {
        UIApplication.shared.applicationState == .active && UIApplication.shared.isProtectedDataAvailable
    }

    /// The Keychain refuses while the phone is locked, as when iOS starts the app early after an update, and can
    /// refuse for a moment after protected data returns.
    private static func unlocked() async {
        let name =
            UIApplication.shared.isProtectedDataAvailable
            ? UIApplication.didBecomeActiveNotification : UIApplication.protectedDataDidBecomeAvailableNotification
        for await _ in NotificationCenter.default.notifications(named: name) { return }
    }

    /// Reads the sign-in from Clerk again, for a new name or picture, or to find it ended elsewhere.
    func refresh() async {
        do {
            guard let active = try await Clerk.client()?.active, let user = active.user else {
                signedOutHere()
                return
            }
            keep(session: active.id, user: user)
        } catch let problem as Clerk.Problem {
            log.error(
                "could not read the sign-in from Clerk: \(problem.status ?? 0) \(problem.code ?? "", privacy: .public) \(problem.message, privacy: .private)"
            )
        } catch {
            log.error("could not read the sign-in from Clerk: \(error, privacy: .public)")
        }
    }

    /// Makes the session a sign-in just created the active one, and the phone signed in.
    func activate(session id: String, client: Clerk.Client?) async throws {
        let known = client?.sessions.first { $0.id == id }
        let user: Clerk.User
        if let found = known?.user {
            user = found
        } else if let found = try await Clerk.client()?.sessions.first(where: { $0.id == id })?.user {
            user = found
        } else {
            throw Clerk.Problem(code: nil, message: String(localized: "Something went wrong; try again."))
        }
        if let jwt = known?.lastActiveToken?.jwt, let expires = Self.expiry(of: jwt) { cached = (jwt, expires) }
        keep(session: id, user: user)
    }

    /// A session token for the accounts server, good for at least a few more seconds. `fresh` asks past the cache.
    func token(fresh: Bool = false) async throws -> String {
        guard let session else {
            throw Clerk.Problem(status: 401, code: nil, message: String(localized: "Sign in again."))
        }
        if !fresh, let cached, cached.expires.timeIntervalSinceNow > 10 { return cached.jwt }
        if let fetching { return try await fetching.value }
        let asking = Task { try await Clerk.token(session: session) }
        fetching = asking
        defer { fetching = nil }
        do {
            let jwt = try await asking.value
            cached = Self.expiry(of: jwt).map { (jwt, $0) }
            return jwt
        } catch let problem as Clerk.Problem where problem.status == 401 || problem.status == 404 {
            signedOutHere()
            throw problem
        }
    }

    /// Ends the session with Clerk and forgets it here. Leaving the account, which also takes the phone off it,
    /// is `Leave`.
    func signOut() async {
        if let session { await Clerk.signOut(session: session) }
        signedOutHere()
    }

    private func signedOutHere() {
        Keychain.app.delete(Self.savedItem)
        Clerk.forget()
        session = nil
        cached = nil
        user = nil
        state = .signedOut
    }

    private func keep(session id: String, user: Clerk.User) {
        do {
            let data = try JSONEncoder().encode(Saved(session: id, user: user))
            try Keychain.app.write(Self.savedItem, String(decoding: data, as: UTF8.self))
        } catch {
            log.error("could not keep the sign-in in the Keychain: \(error, privacy: .public)")
        }
        apply(session: id, user: user)
    }

    private func apply(session id: String, user: Clerk.User) {
        if session != id { cached = nil }
        session = id
        if self.user != user { self.user = user }
        if state != .signedIn(userId: user.id) { state = .signedIn(userId: user.id) }
    }

    private static func expiry(of jwt: String) -> Date? {
        let parts = jwt.split(separator: ".")
        guard parts.count > 1 else { return nil }
        guard let data = Data(base64URL: String(parts[1])),
            let claims = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
            let exp = claims["exp"] as? Double
        else { return nil }
        return Date(timeIntervalSince1970: exp)
    }
}

extension Clerk.User {
    var name: String? {
        let full = [firstName, lastName].compactMap { $0 }.joined(separator: " ").trimmingCharacters(in: .whitespaces)
        return full.isEmpty ? nil : full
    }

    var email: String? {
        (emailAddresses.first { $0.id == primaryEmailAddressId } ?? emailAddresses.first)?.emailAddress
    }

    /// The name of the provider the account signs in with, as Clerk calls it ("oauth_google"); none for email.
    var providerName: String? {
        guard let provider = externalAccounts?.first?.provider else { return nil }
        let id = provider.replacingOccurrences(of: "oauth_", with: "")
        return ["google": "Google", "github": "GitHub", "apple": "Apple"][id] ?? id
    }

    var signsInWithApple: Bool {
        externalAccounts?.contains { $0.provider.hasSuffix("apple") } ?? false
    }
}
