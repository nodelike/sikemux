import Foundation
import Network
import Observation

/// How far adding this phone to the account has got, so a screen can say why it stopped.
enum AccountStatus: Equatable {
    case registering
    case registered
    case failed(problem: String, retrying: Bool)
    case deleting(deletesAt: String)
}

/// Why this phone signed out: removed from the account, the account deleted elsewhere, or deleted here.
enum Farewell {
    case removed, deleted, deletedHere
}

/// Puts this phone on the account while signed in, and keeps it connected while the app is in front: registration,
/// then the live connection, which the server only takes from a phone on the account. A failed registration tries
/// again with backoff while the app is in front, and at once when it comes back to the front or the network returns.
@MainActor @Observable
final class AccountSession {
    static let shared = AccountSession()

    /// Nil while signed out.
    private(set) var status: AccountStatus?
    /// Said once on the way out, before Welcome.
    var farewell: Farewell?
    /// A device that just joined the account, for the person to keep or take off (ADR 0018).
    var newDevice: WireDevice?
    /// Why taking that device off failed.
    var removeProblem: String?

    @ObservationIgnored private var userId: String?
    @ObservationIgnored private var live: LiveAccount?
    @ObservationIgnored private var registered = false
    @ObservationIgnored private var registering = false
    @ObservationIgnored private var front = true
    @ObservationIgnored private var attempt = 0
    @ObservationIgnored private var retrying: Task<Void, Never>?
    @ObservationIgnored private let path = NWPathMonitor()
    @ObservationIgnored private var wasOnline = true

    private init() {
        path.pathUpdateHandler = { [weak self] update in
            let online = update.status == .satisfied
            Task { @MainActor in self?.network(online) }
        }
        path.start(queue: .main)
    }

    /// Exponential backoff with half of it random, so clients that lost the server together do not return together.
    nonisolated static func backoff(_ attempt: Int) -> TimeInterval {
        let ceiling = min(60, pow(2, Double(attempt)))
        return ceiling / 2 + ceiling / 2 * Double.random(in: 0..<1)
    }

    /// Follows who is signed in: starts the sequence for a new account, ends it on signing out.
    func follow(_ userId: String?) {
        guard userId != self.userId else { return }
        end()
        self.userId = userId
        guard userId != nil else { return }
        live = LiveAccount(
            handlers: .init(
                hostsChanged: { AccountHosts.shared.read() },
                stashChanged: { StashStore.shared.changed() },
                deviceAdded: { key in Task { await AccountSession.shared.askAbout(key) } },
                ready: { AccountHosts.shared.retry() },
                gone: { reason in
                    Task { await Leave.signOutHere(farewell: reason.farewell, confirmed: true) }
                }
            ))
        AccountHosts.shared.read()
        register()
    }

    func foreground() {
        front = true
        connectArrivals()
        if registered { live?.start() } else { retryNow() }
    }

    func background() {
        front = false
        retrying?.cancel()
        live?.stop()
    }

    /// Tries adding this phone to the account again now, as the person asked.
    func retryNow() {
        guard userId != nil, !registered, !registering else { return }
        attempt = 0
        register()
    }

    private func network(_ online: Bool) {
        defer { wasOnline = online }
        guard online, !wasOnline, userId != nil else { return }
        if !registered { return retryNow() }
        live?.nudge()
        AccountHosts.shared.retry()
    }

    private func end() {
        retrying?.cancel()
        live?.stop()
        live = nil
        registered = false
        registering = false
        status = nil
        newDevice = nil
        AccountHosts.shared.clear()
    }

    private func register() {
        guard let userId, !registering else { return }
        retrying?.cancel()
        registering = true
        status = .registering
        Task {
            do {
                let phone = try await AccountCalls.registerPhone(userId: userId)
                registering = false
                guard self.userId == userId else { return }
                registered = true
                status = .registered
                if front { live?.start() }
                await Notify.shared.phoneAdded(at: phone.createdAt)
            } catch {
                registering = false
                guard self.userId == userId else { return }
                if let deleting = error as? AccountDeleting {
                    status = .deleting(deletesAt: deleting.deletesAt)
                    return
                }
                let problem = error as? ApiProblem
                let retryable = problem == nil || problem!.unreachable || problem!.status == 429
                status = .failed(problem: Sikemux.problem(error), retrying: retryable)
                guard retryable, front else { return }
                let wait = Self.backoff(attempt)
                attempt += 1
                retrying = Task {
                    try? await Task.sleep(for: .seconds(wait))
                    if !Task.isCancelled { register() }
                }
            }
        }
    }

    /// Starts connecting to a host that signed in to the account after this phone did. A host that arrives while the
    /// phone is in the background stays new until the phone is in front, so it still connects then.
    func connectArrivals() {
        guard let userId, front, AccountHosts.shared.loaded, Joining.screens == 0 else { return }
        let hosts = AccountHosts.shared.hosts
        Task {
            let arrived = await Arrivals.shared.hostsArrived(account: userId, hosts: hosts)
            guard let host = arrived.first(where: { found in !Hub.shared.devices.contains { $0.core == found.key } })
            else { return }
            Router.shared.open(.join(core: host.key, name: host.name, arrived: true))
        }
    }

    func remove(_ device: WireDevice) {
        Task {
            do {
                try await AccountCalls.removeDevice(device.key)
            } catch {
                removeProblem = Sikemux.problem(error)
            }
        }
    }

    private func askAbout(_ key: String) async {
        do {
            newDevice = try await AccountCalls.device(key)
        } catch {
            print("sikemux: could not read the device new to the account: \(error)")
        }
    }
}

extension RevokeReason {
    /// Signing out on this phone needs no explanation; anything else does.
    var farewell: Farewell? {
        switch self {
        case .removed: .removed
        case .accountDeleted: .deleted
        case .signedOut, .unknown: nil
        }
    }
}

/// The hosts on the account, read on signing in and again whenever the live connection hears one change.
@MainActor @Observable
final class AccountHosts {
    static let shared = AccountHosts()

    private(set) var hosts: [WireDevice] = []
    /// The server has answered since signing in.
    private(set) var loaded = false
    /// Why the last read failed, while it has.
    private(set) var problem: String?
    @ObservationIgnored private var reading = 0

    func read() {
        reading += 1
        let current = reading
        Task {
            do {
                let found = try await AccountCalls.hosts()
                guard current == reading else { return }
                hosts = found
                loaded = true
                problem = nil
                AccountSession.shared.connectArrivals()
            } catch {
                guard current == reading else { return }
                problem = Sikemux.problem(error)
            }
        }
    }

    /// Reads the hosts again, unless the last read worked; the live connection keeps a good list current.
    func retry() {
        if !loaded || problem != nil { read() }
    }

    fileprivate func clear() {
        reading += 1
        hosts = []
        loaded = false
        problem = nil
    }
}

/// The phone's half of leaving the account.
@MainActor
enum Leave {
    private static var leaving: Task<Void, Never>?

    /// Forgets every paired host, asking each one it can reach to unpair it, then signs out of Clerk. Without
    /// `confirmed`, the server may still have this phone, so it comes back as a new device. Two callers at once share
    /// one run.
    static func signOutHere(farewell: Farewell? = nil, confirmed: Bool) async {
        if let farewell, !(AccountSession.shared.farewell == .deletedHere && farewell == .deleted) {
            AccountSession.shared.farewell = farewell
        }
        if let leaving { return await leaving.value }
        let run = Task {
            AccountSession.shared.follow(nil)
            await Notify.shared.forget()
            ShareExtension.signOut()
            StashStore.shared.forget()
            await Lock.shared.turnOff()
            await withTaskGroup(of: Void.self) { group in
                for device in Hub.shared.devices {
                    group.addTask { await Hub.shared.forget(device.core) }
                }
            }
            LiveAccount.forgetCursor()
            if !confirmed { await Identity.shared.rotateKey() }
            await Account.shared.signOut()
            Router.shared.root()
        }
        leaving = run
        await run.value
        leaving = nil
    }

    /// Takes this phone off the account first, so hosts hear of it, then leaves. Throws when the server could not
    /// confirm, so the person can choose to leave anyway.
    static func signOut() async throws {
        try await Notify.shared.stopPush()
        try await AccountCalls.removePhone()
        await signOutHere(confirmed: true)
    }
}
