import CryptoKit
import Foundation
import Observation
import SikemuxCore
import UIKit
import UserNotifications

/// Whether the person wants notifications and whether the system lets the app show them; the account's copy of
/// this phone's push token; and the keys hosts seal cards with.
@MainActor @Observable
final class Notify {
    enum Choice: String {
        case on, off
    }

    static let shared = Notify()

    private static let choiceItem = "sikemux.notifications"
    private static let sentItem = "sikemux.push-token"
    /// What the phone asks every host for until it has its own settings screen.
    private static let prefs = NotifyPrefs(needsYou: true, finished: true, problems: true, when: .away, muted: [])
    /// APNs tokens can go quiet without changing; sending one again now and then keeps the server's copy fresh.
    private static let resend: TimeInterval = 30 * 24 * 60 * 60

    /// Nil until the person first chooses.
    private(set) var choice: Choice?
    /// Nil until the system has asked the person.
    private(set) var allowed: Bool?
    /// Asked once, after a host first lets a signed-in phone in.
    var offered = false

    @ObservationIgnored private var token: CheckedContinuation<Data, Error>?
    @ObservationIgnored private var syncing: Task<Void, Error>?

    private init() {
        do {
            choice = try Keychain.read(Self.choiceItem).flatMap(Choice.init)
        } catch {
            print("sikemux: could not read the notifications choice: \(error)")
        }
        UNUserNotificationCenter.current().setNotificationCategories([Self.answers])
        NotificationCenter.default.addObserver(forName: UIApplication.didBecomeActiveNotification, object: nil, queue: .main) { _ in
            Task { await Notify.shared.checkAllowed() }
        }
    }

    var on: Bool { choice == .on && allowed == true }
    /// On, but the system blocks them, so the way to Settings shows.
    var blocked: Bool { choice == .on && allowed == false }

    func checkAllowed() async {
        let status = await UNUserNotificationCenter.current().notificationSettings().authorizationStatus
        allowed = status == .notDetermined ? nil : status == .authorized || status == .provisional
    }

    private func choose(_ next: Choice?) {
        choice = next
        guard let next else { return Keychain.delete(Self.choiceItem) }
        do {
            try Keychain.write(Self.choiceItem, next.rawValue)
        } catch {
            print("sikemux: could not keep the notifications choice: \(error)")
        }
    }

    /// Asks once, after a host lets the phone in, whether to turn notifications on; a person who already chose is not asked.
    func offer() {
        if choice == nil { offered = true }
    }

    func declineOffer() {
        offered = false
        choose(.off)
    }

    /// Turns notifications on, asking the system first.
    func turnOn() async {
        offered = false
        choose(.on)
        do {
            allowed = try await UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound, .badge])
        } catch {
            print("sikemux: could not ask to notify: \(error)")
            allowed = false
        }
        guard allowed == true else { return }
        do { try await syncPushToken() } catch { print("sikemux: could not register for notifications: \(error)") }
        for (core, connection) in Hub.shared.openConnections { Task { await shareKey(core, connection) } }
    }

    /// Every connected host stops, the cards and keys go, and the server forgets the token.
    func turnOff() async {
        choose(.off)
        for (core, connection) in Hub.shared.openConnections { await withdrawKey(core, connection) }
        Keys().clear()
        UNUserNotificationCenter.current().removeAllDeliveredNotifications()
        do { try await stopPush() } catch { print("sikemux: could not take back the notification token: \(error)") }
    }

    /// Forgets everything notifications kept on this phone, as signing out does, and drops the system's token, so a
    /// server that never heard of the sign-out has nothing left to send to.
    func forget() async {
        Keys().clear()
        UNUserNotificationCenter.current().removeAllDeliveredNotifications()
        Keychain.delete(Self.sentItem)
        choose(nil)
        UIApplication.shared.unregisterForRemoteNotifications()
    }

    // MARK: The push token

    /// Which build the token belongs to: dev builds have their own bundle id and reach a server that serves only them.
    fileprivate enum PushApp: String, Codable {
        case dev, production
    }

    /// Which of Apple's push servers issued the token.
    fileprivate enum ApnsEnvironment: String, Codable {
        case sandbox, production
    }

    /// What the server last took from this phone, so an unchanged token is not sent again.
    private struct SentToken: Codable {
        var device: String
        var tokenSha256: String
        var app: PushApp
        var apnsEnvironment: ApnsEnvironment
        /// Milliseconds since 1970.
        var at: Double = 0

        func same(as other: SentToken) -> Bool {
            device == other.device && tokenSha256 == other.tokenSha256 && app == other.app
                && apnsEnvironment == other.apnsEnvironment
        }
    }

    /// A copy that cannot be read counts as none, so the token is sent again.
    private func readSent() -> SentToken? {
        do {
            guard let stored = try Keychain.read(Self.sentItem) else { return nil }
            return try JSONDecoder().decode(SentToken.self, from: Data(stored.utf8))
        } catch {
            print("sikemux: could not read the push token the server has: \(error)")
            return nil
        }
    }

    private func keepSent(_ token: SentToken?) throws {
        guard let token else { return Keychain.delete(Self.sentItem) }
        try Keychain.write(Self.sentItem, String(decoding: try JSONEncoder().encode(token), as: UTF8.self))
    }

    /// Makes the server's copy of this phone's token match what the person chose and the system allows: sends it
    /// when it is new, changed or old, and takes it back when notifications are off.
    func syncPushToken() async throws {
        if let syncing { return try await syncing.value }
        let work = Task { try await sync() }
        syncing = work
        defer { syncing = nil }
        try await work.value
    }

    private func sync() async throws {
        await checkAllowed()
        let sent = readSent()
        guard choice == .on, allowed == true else {
            if sent != nil { try await stopPush() }
            return
        }
        let identity = try Identity.shared.deviceIdentity()
        Keys().phone = identity.id()
        let pushToken = try await deviceToken().map { String(format: "%02x", $0) }.joined()
        let tokenSha256 = SHA256.hash(data: Data(pushToken.utf8)).map { String(format: "%02x", $0) }.joined()
        var next = SentToken(device: identity.id(), tokenSha256: tokenSha256, app: Config.isDev ? .dev : .production,
                             apnsEnvironment: Self.apnsEnvironment)
        if let sent, sent.same(as: next), Date().timeIntervalSince1970 - sent.at / 1000 < Self.resend { return }
        let nonce = try await AccountCalls.challenge()
        let registration = PushRegistration(
            token: pushToken, app: next.app, apnsEnvironment: next.apnsEnvironment, nonce: nonce,
            signature: try identity.signPush(nonce: nonce, tokenSha256: tokenSha256)
        )
        let _: PushTokenState = try await AccountAPI.send("PUT", "/v1/devices/\(identity.id())/push", body: registration)
        next.at = Date().timeIntervalSince1970 * 1000
        try keepSent(next)
    }

    /// Takes this phone's token off the server. Throws when the server can't be reached, so signing out can ask first.
    func stopPush() async throws {
        let device = try Identity.shared.deviceIdentity().id()
        do {
            try await AccountAPI.send("DELETE", "/v1/devices/\(device)/push")
        } catch let failure as ApiProblem where failure.status == 404 {}
        try keepSent(nil)
    }

    private func deviceToken() async throws -> Data {
        try await withCheckedThrowingContinuation { waiting in
            token?.resume(throwing: CancellationError())
            token = waiting
            UIApplication.shared.registerForRemoteNotifications()
        }
    }

    func registered(_ result: Result<Data, Error>) {
        token?.resume(with: result)
        token = nil
    }

    /// The simulator and builds signed for development get their tokens from Apple's sandbox; App Store and
    /// TestFlight builds carry no profile.
    private static let apnsEnvironment: ApnsEnvironment = {
        #if targetEnvironment(simulator)
        return .sandbox
        #else
        guard let url = Bundle.main.url(forResource: "embedded", withExtension: "mobileprovision") else { return .production }
        do {
            let signed = try String(contentsOf: url, encoding: .isoLatin1)
            guard let start = signed.range(of: "<?xml"), let end = signed.range(of: "</plist>") else { return .sandbox }
            let profile = try PropertyListSerialization.propertyList(
                from: Data(signed[start.lowerBound..<end.upperBound].utf8), format: nil) as? [String: Any]
            let entitlements = profile?["Entitlements"] as? [String: Any]
            return entitlements?["aps-environment"] as? String == "production" ? .production : .sandbox
        } catch {
            print("sikemux: could not read this build's provisioning profile: \(error)")
            return .sandbox
        }
        #endif
    }()

    // MARK: Host keys

    /// The key this phone gave `host`, or a new one when it has none or the host's last push would not open with it.
    private func hostKey(_ host: String) -> HostKey {
        let keys = Keys()
        if let kept = keys.get(host: host) { return kept }
        let key = SymmetricKey(size: .bits256).withUnsafeBytes { Data($0) }
        let keyId = UInt32.random(in: .min ... .max)
        keys.put(host: host, keyId: keyId, key: key)
        return HostKey(host: host, keyId: keyId, key: key)
    }

    /// Gives a host this phone's notification key on each connection, while notifications are on. A host older than
    /// notifications never answers.
    func shareKey(_ host: String, _ connection: Connection) async {
        guard choice == .on else { return }
        let key = hostKey(host)
        do {
            let prefs = String(decoding: try JSONEncoder().encode(Self.prefs), as: UTF8.self)
            try await connection.setNotifications(keyId: key.keyId, key: key.key, prefsJson: prefs)
        } catch {
            print("sikemux: could not give the host the notification key: \(error)")
        }
    }

    /// Asks a host to stop notifying this phone, and forgets the key it had.
    func withdrawKey(_ host: String, _ connection: Connection?) async {
        Keys().remove(host: host)
        do {
            try await connection?.clearNotifications()
        } catch {
            print("sikemux: could not ask the host to stop notifying: \(error)")
        }
    }

    // MARK: Card buttons

    /// Reject and Allow open the app, which answers over its own connection to the host; iOS gives an app woken in
    /// the background too little time to reach one. Allow asks for the phone's lock first.
    private static let answers = UNNotificationCategory(
        identifier: Card.answerCategory,
        actions: [
            UNNotificationAction(identifier: CardAction.reject.rawValue, title: "Reject", options: [.foreground, .destructive]),
            UNNotificationAction(identifier: CardAction.allow.rawValue, title: "Allow", options: [.foreground, .authenticationRequired]),
        ],
        intentIdentifiers: []
    )
}

private struct PushRegistration: Encodable {
    let platform = "apns"
    let token: String
    let app: Notify.PushApp
    let apnsEnvironment: Notify.ApnsEnvironment
    let nonce: String
    let signature: String
}

private struct PushTokenState: Decodable {
    let enabled: Bool
}
