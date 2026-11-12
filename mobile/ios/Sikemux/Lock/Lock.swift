import LocalAuthentication
import Observation
import UIKit

/// Asks for Face ID, Touch ID or the passcode before the app shows an account's hosts and chats, when the person
/// turned it on. Signed out there is nothing behind it, and someone who signs in on the phone has just proved who
/// they are.
@MainActor @Observable
final class Lock {
    /// open: showing the app. covered: away or behind a system sheet, so the app switcher's picture shows nothing.
    /// locked: waiting for Face ID, Touch ID or the passcode.
    enum Phase {
        case open, covered, locked
    }

    static let shared = Lock()

    /// How long the app may be away before it asks again, as password managers allow a quick switch to another app.
    private static let away: TimeInterval = 60
    private static let item = "sikemux.lock"

    private(set) var phase: Phase
    private(set) var asking = false
    /// The person's choice, off until they turn it on.
    private(set) var on: Bool
    /// The app has shown itself since it started.
    private(set) var opened = false

    @ObservationIgnored private var locked = true
    @ObservationIgnored private var signedIn: Bool?
    @ObservationIgnored private var leftAt: Date?

    private init() {
        on = Self.chosen() ?? true
        phase = UIApplication.shared.applicationState == .active ? .locked : .covered
        Router.shared.holding = true
        let center = NotificationCenter.default
        center.addObserver(forName: UIApplication.didBecomeActiveNotification, object: nil, queue: .main) { _ in
            MainActor.assumeIsolated { Lock.shared.changed(active: true, background: false) }
        }
        center.addObserver(forName: UIApplication.willResignActiveNotification, object: nil, queue: .main) { _ in
            MainActor.assumeIsolated { Lock.shared.changed(active: false, background: false) }
        }
        center.addObserver(forName: UIApplication.didEnterBackgroundNotification, object: nil, queue: .main) { _ in
            MainActor.assumeIsolated { Lock.shared.changed(active: false, background: true) }
        }
    }

    /// Settles the lock once the account is known: a phone signed in when the app starts asks first if the lock is on.
    func follow(signedIn account: Bool) {
        guard account != signedIn else { return }
        let launched = signedIn == nil
        signedIn = account
        if launched { on = Self.chosen() ?? on }
        if account, launched, on { return }
        open()
    }

    /// Asks for Face ID, Touch ID or the phone's passcode, and opens the app once the person passes.
    func unlock() async {
        guard locked, !asking, UIApplication.shared.applicationState == .active else { return }
        asking = true
        // A phone without a passcode has nothing to ask for, and anyone holding it already has everything on it.
        switch await authenticate(String(localized: "Unlock Sikemux")) {
        case .passed, .noPasscode: open()
        case .failed:
            phase = .locked
            asking = false
        }
    }

    /// Turns the lock on once the person passes the prompt, so it is known to work; answers why when it can't.
    func turnOn() async -> String? {
        asking = true
        let result = await authenticate(String(localized: "Lock Sikemux"))
        asking = false
        switch result {
        case .passed:
            do {
                try Keychain.app.write(Self.item, "on")
            } catch {
                return Sikemux.problem(error)
            }
            on = true
            Glances.changed()
            return nil
        case .noPasscode: return String(localized: "Set a passcode in Settings first")
        case .failed: return nil
        }
    }

    func turnOff() async {
        Keychain.app.delete(Self.item)
        on = false
        Glances.changed()
    }

    /// Nil when the Keychain refuses, as it does while the phone is locked; the app then stays locked.
    private static func chosen() -> Bool? {
        do {
            return try Keychain.app.read(item) == "on"
        } catch {
            print("sikemux: could not read whether the lock is on: \(error)")
            return nil
        }
    }

    private enum Result {
        case passed, noPasscode, failed
    }

    private func authenticate(_ reason: String) async -> Result {
        let context = LAContext()
        do {
            return try await context.evaluatePolicy(.deviceOwnerAuthentication, localizedReason: reason)
                ? .passed : .failed
        } catch let error as LAError where error.code == .passcodeNotSet || error.code == .biometryNotEnrolled {
            return .noPasscode
        } catch {
            return .failed
        }
    }

    private func open() {
        locked = false
        leftAt = nil
        set(UIApplication.shared.applicationState == .active || !on ? .open : .covered)
        asking = false
        opened = true
    }

    private func set(_ next: Phase) {
        phase = next
        if next == .open { Router.shared.release() } else { Router.shared.holding = true }
    }

    private func changed(active: Bool, background: Bool) {
        guard !asking, on else { return }
        if !active {
            if background, leftAt == nil { leftAt = Date() }
            if !locked { set(.covered) }
            return
        }
        let returned = leftAt != nil
        if signedIn == true, let leftAt, Date().timeIntervalSince(leftAt) >= Self.away { locked = true }
        leftAt = nil
        guard locked else { return set(.open) }
        set(.locked)
        // Coming back from the system's own prompt or a pulled-down notification centre is not a return to the app.
        if returned { Task { await unlock() } }
    }
}
