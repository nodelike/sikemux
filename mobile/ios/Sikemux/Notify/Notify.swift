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
    private static let prefs = #"{"needsYou":true,"finished":true,"problems":true,"when":"away","muted":[]}"#
    /// A host older than notifications never answers, so its silence means it needs an update.
    private static let answerWait: Duration = .seconds(10)

    /// Undefined until the person first chooses.
    private(set) var choice: Choice?
    private(set) var allowed: Bool?
    /// Asked once, after a host first lets a signed-in phone in.
    var offered = false

    @ObservationIgnored private var token: CheckedContinuation<Data, Error>?
    @ObservationIgnored private var syncing: Task<Void, Error>?

    private init() {
        choice = (try? Keychain.read(Self.choiceItem)).flatMap { $0.flatMap(Choice.init) }
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
        if let next { try? Keychain.write(Self.choiceItem, next.rawValue) } else { Keychain.delete(Self.choiceItem) }
    }

    /// Asks once, after a host lets the phone in, whether to turn notifications on; a person who already chose is not asked.
    func offer() {
        if choice == nil { offered = true }
    }

    func declineOffer() {
        offered = false
        choose(.off)
    }

    /// Turns notifications on, asking the system first; answers whether the system allows them.
    @discardableResult
    func turnOn() async -> Bool {
        offered = false
        choose(.on)
        let granted = (try? await UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound, .badge])) ?? false
        allowed = granted
        guard granted else { return false }
        do { try await syncPushToken() } catch { print("sikemux: could not register for notifications: \(error)") }
        shareKeys()
        return true
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

    private struct SentToken: Codable, Equatable {
        var device: String
        var tokenSha256: String
        var app: String
        var apnsEnvironment: String
        var at: Double = 0

        func same(as other: SentToken) -> Bool {
            device == other.device && tokenSha256 == other.tokenSha256 && app == other.app
                && apnsEnvironment == other.apnsEnvironment
        }
    }

    /// APNs tokens can go quiet without changing; sending one again now and then keeps the server's copy fresh.
    private static let resend: TimeInterval = 30 * 24 * 60 * 60

    private var sent: SentToken? {
        get { (try? Keychain.read(Self.sentItem)).flatMap { $0 }.flatMap { try? JSONDecoder().decode(SentToken.self, from: Data($0.utf8)) } }
        set {
            if let newValue, let data = try? JSONEncoder().encode(newValue) {
                try? Keychain.write(Self.sentItem, String(decoding: data, as: UTF8.self))
            } else {
                Keychain.delete(Self.sentItem)
            }
        }
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
        guard choice == .on, allowed == true else {
            if sent != nil { try await stopPush() }
            return
        }
        let identity = try Identity.shared.deviceIdentity()
        Keys().phone = identity.id()
        let pushToken = try await deviceToken().map { String(format: "%02x", $0) }.joined()
        let tokenSha256 = SHA256.hash(data: Data(pushToken.utf8)).map { String(format: "%02x", $0) }.joined()
        var next = SentToken(device: identity.id(), tokenSha256: tokenSha256, app: Config.isDev ? "dev" : "production",
                             apnsEnvironment: Self.apnsEnvironment)
        if let sent, sent.same(as: next), Date().timeIntervalSince1970 - sent.at / 1000 < Self.resend { return }
        let challenge: Challenge = try await AccountAPI.send("POST", "/v1/devices/challenge")
        let registration = PushRegistration(
            token: pushToken, app: next.app, apnsEnvironment: next.apnsEnvironment, nonce: challenge.nonce,
            signature: try identity.signPush(nonce: challenge.nonce, tokenSha256: tokenSha256)
        )
        let _: PushTokenState = try await AccountAPI.send("PUT", "/v1/devices/\(identity.id())/push", body: registration)
        next.at = Date().timeIntervalSince1970 * 1000
        sent = next
    }

    /// Takes this phone's token off the server. Throws when the server can't be reached, so signing out can ask first.
    func stopPush() async throws {
        let device = try Identity.shared.deviceIdentity().id()
        do {
            try await AccountAPI.send("DELETE", "/v1/devices/\(device)/push")
        } catch let failure as ApiProblem where failure.status == 404 {}
        sent = nil
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

    /// Which of Apple's push servers issued this build's token. The simulator and builds signed for development get
    /// theirs from the sandbox; App Store and TestFlight builds carry no profile.
    private static let apnsEnvironment: String = {
        #if targetEnvironment(simulator)
        return "sandbox"
        #else
        guard let url = Bundle.main.url(forResource: "embedded", withExtension: "mobileprovision"),
              let signed = try? String(contentsOf: url, encoding: .isoLatin1)
        else { return "production" }
        guard let start = signed.range(of: "<?xml"), let end = signed.range(of: "</plist>"),
              let profile = try? PropertyListSerialization.propertyList(
                  from: Data(signed[start.lowerBound..<end.upperBound].utf8), format: nil) as? [String: Any],
              let entitlements = profile["Entitlements"] as? [String: Any]
        else { return "sandbox" }
        return entitlements["aps-environment"] as? String == "production" ? "production" : "sandbox"
        #endif
    }()

    // MARK: Host keys

    enum Shared {
        case shared, off, hostTooOld, failed
    }

    /// The key this phone gave `host`, or a new one when it has none or the host's last push would not open with it.
    private func hostKey(_ host: String) -> HostKey {
        let keys = Keys()
        if let kept = keys.get(host: host) { return kept }
        var random = [UInt8](repeating: 0, count: 36)
        _ = SecRandomCopyBytes(kSecRandomDefault, random.count, &random)
        let keyId = random[0..<4].reduce(UInt32(0)) { $0 << 8 | UInt32($1) }
        let key = Data(random[4...])
        keys.put(host: host, keyId: keyId, key: key)
        return HostKey(host: host, keyId: keyId, key: key)
    }

    /// Gives a host this phone's notification key on each connection, while notifications are on.
    @discardableResult
    func shareKey(_ host: String, _ connection: Connection) async -> Shared {
        guard choice == .on else { return .off }
        let key = hostKey(host)
        let answered = await withTaskGroup(of: Shared?.self) { group in
            group.addTask {
                do {
                    try await connection.setNotifications(keyId: key.keyId, key: key.key, prefsJson: Self.prefs)
                    return .shared
                } catch {
                    print("sikemux: could not give the host the notification key: \(error)")
                    return .failed
                }
            }
            group.addTask {
                try? await Task.sleep(for: Self.answerWait)
                return Task.isCancelled ? nil : .hostTooOld
            }
            let first = await group.next() ?? nil
            group.cancelAll()
            return first ?? .failed
        }
        return answered
    }

    private func shareKeys() {
        for (core, connection) in Hub.shared.openConnections { Task { await shareKey(core, connection) } }
    }

    /// Asks a host to stop notifying this phone, and forgets the key it had.
    func withdrawKey(_ host: String, _ connection: Connection?) async {
        Keys().remove(host: host)
        try? await connection?.clearNotifications()
    }

    // MARK: Card buttons

    /// Reject and Allow open the app, which answers over its own connection to the host; iOS gives an app woken in
    /// the background too little time to reach one. Allow asks for the phone's lock first.
    private static let answers = UNNotificationCategory(
        identifier: Card.answerCategory,
        actions: [
            UNNotificationAction(identifier: "reject", title: "Reject", options: [.foreground, .destructive]),
            UNNotificationAction(identifier: "allow", title: "Allow", options: [.foreground, .authenticationRequired]),
        ],
        intentIdentifiers: []
    )
}

private struct Challenge: Decodable {
    let nonce: String
}

private struct PushRegistration: Encodable {
    let platform = "apns"
    let token: String
    let app: String
    let apnsEnvironment: String
    let nonce: String
    let signature: String
}

private struct PushTokenState: Decodable {
    let enabled: Bool
}
