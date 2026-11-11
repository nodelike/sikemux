import Foundation
import SikemuxCore
import UIKit

/// Why joining a host stopped, in words for the person. A `held` host is not joined again until the person asks.
struct JoinFailure: Error, Equatable, Codable {
    let title: String
    let detail: String
    var held = false
}

/// Joining a host on the account: a ticket from Sikemux, handed to the host, then waiting while it lets the phone in,
/// at once or once someone there allows it.
@MainActor
enum Joining {
    /// Joins the account's host `core`; an allowed phone keeps the host as paired.
    static func join(core: String, name: String) async throws {
        let ticket: JoinTicket
        do {
            ticket = try await AccountCalls.joinTicket(host: core)
        } catch {
            throw ticketFailure(name, error)
        }
        try Task.checkCancellation()
        let asked = Date()
        let encoded = String(decoding: try JSONEncoder().encode(ticket), as: UTF8.self)
        let answer: JoinAnswer
        do {
            answer = try await BackgroundTime.run("Joining a host") {
                try await Identity.shared.whileJoining { device in
                    try await device.join(core: core, ticket: encoded, name: UIDevice.current.name, platform: "ios")
                }
            }
        } catch {
            throw dialFailure(name, error)
        }
        try Task.checkCancellation()
        switch answer {
        case .denied where Date().timeIntervalSince(asked) >= TimeInterval(WireWaits.approvalTimeoutSeconds):
            throw JoinFailure(
                title: "\(name) did not answer",
                detail: "Nobody allowed this phone within \(WireWaits.approvalTimeoutSeconds / 60) minutes.")
        case .denied:
            throw JoinFailure(
                title: "\(name) said no", detail: "Someone at \(name) turned this phone down.", held: true)
        case .refused(let reason):
            throw refusal(name, reason)
        case .allowed(let access):
            await Hub.shared.remember(
                PairedDevice(
                    core: core, access: DeviceAccess(rawValue: access) ?? .unknown,
                    pairedAt: Date().timeIntervalSince1970 * 1000, name: name))
            Hub.shared.rejoined(core)
        }
    }

    /// Why the host refused a ticket, in the words sikemux_core::join::Refusal uses.
    private static func refusal(_ host: String, _ reason: String) -> JoinFailure {
        let why = [
            "signed_out": "\(host) is not signed in to Sikemux. Sign it in, then try again.",
            "wrong_account": "\(host) is signed in to a different account. Sign it in to this one, then try again.",
            "revoked": "This phone was removed from your account. Sign in again.",
            "expired": "Check the clock on \(host) and on this phone, then try again.",
            "not_yet_valid": "Check the clock on \(host) and on this phone, then try again.",
            "unknown_key": "\(host) does not trust this Sikemux server. Update it, then try again.",
        ]
        return JoinFailure(
            title: "\(host) couldn't let this phone in",
            detail: why[reason] ?? "It turned down the invitation (\(reason)). Try again."
        )
    }

    private static func ticketFailure(_ host: String, _ error: Error) -> JoinFailure {
        guard let problem = error as? ApiProblem else {
            return JoinFailure(title: "Sikemux couldn't invite this phone", detail: Sikemux.problem(error))
        }
        if problem.status == 404 {
            return JoinFailure(
                title: "\(host) is no longer on your account", detail: "Sign it in to this account, then try again.")
        }
        if problem.status == 429 {
            return JoinFailure(title: "Too many tries", detail: "Wait a minute, then try again.")
        }
        if problem.unreachable {
            return JoinFailure(title: "Can't reach Sikemux", detail: "Check the phone is online, then try again.")
        }
        return JoinFailure(title: "Sikemux couldn't invite this phone", detail: problem.message)
    }

    private static func dialFailure(_ host: String, _ error: Error) -> JoinFailure {
        switch error as? MobileError {
        case .Connection:
            return JoinFailure(
                title: "Can't reach \(host)", detail: "It may be asleep, offline, or have remote access turned off.")
        case .Invalid(let message):
            return JoinFailure(title: "Sikemux's invitation didn't fit", detail: message)
        default:
            return JoinFailure(title: "Connecting stopped", detail: Sikemux.problem(error))
        }
    }
}

/// Joins every host on the account this phone has not paired with, as soon as the account lists it (ADR 0018). A join
/// that stops waits on its card until the person tries again; one turned down or forgotten here stays stopped across
/// launches.
@MainActor @Observable
final class HostJoins {
    enum State: Equatable {
        case joining(since: Date)
        case stopped(JoinFailure)
    }

    private struct Held: Codable {
        var account: String
        var hosts: [String: JoinFailure]
    }

    static let shared = HostJoins()
    /// A host that has not answered by now is most likely waiting for someone there to allow the phone.
    static let askingAfter: TimeInterval = 5

    private(set) var states: [String: State] = [:]
    @ObservationIgnored private var account: String?
    @ObservationIgnored private var running: [String: Task<Void, Never>] = [:]
    @ObservationIgnored private let file = URL.documentsDirectory.appending(path: "held-hosts.json")

    func follow(_ account: String?) {
        guard account != self.account else { return }
        for task in running.values { task.cancel() }
        running = [:]
        self.account = account
        states = (account.flatMap(readHeld) ?? [:]).mapValues { .stopped($0) }
        joinNew()
    }

    /// Starts joining each listed host that is neither paired nor already joined or stopped.
    func joinNew() {
        let hosts = AccountHosts.shared
        guard account != nil, Hub.shared.loaded, hosts.loaded, UIApplication.shared.applicationState != .background
        else { return }
        for host in hosts.hosts where states[host.key] == nil && Hub.shared.device(host.key) == nil {
            join(host)
        }
    }

    func retry(_ host: WireDevice) {
        guard running[host.key] == nil else { return }
        join(host)
    }

    /// The person forgot this host here, so it is not joined again by itself.
    func forgot(_ core: String) {
        running.removeValue(forKey: core)?.cancel()
        stop(
            core,
            JoinFailure(title: "Forgotten on this phone", detail: "Try again to use it here.", held: true))
    }

    private func join(_ host: WireDevice) {
        let account = self.account
        states[host.key] = .joining(since: .now)
        saveHeld()
        running[host.key] = Task {
            do {
                try await Joining.join(core: host.key, name: host.name)
                guard !Task.isCancelled, self.account == account else { return }
                running[host.key] = nil
                states[host.key] = nil
                Notify.shared.offer()
            } catch {
                guard !Task.isCancelled, self.account == account else { return }
                running[host.key] = nil
                stop(
                    host.key,
                    error as? JoinFailure ?? JoinFailure(title: "Connecting stopped", detail: Sikemux.problem(error)))
            }
        }
    }

    private func stop(_ core: String, _ failure: JoinFailure) {
        guard account != nil else { return }
        states[core] = .stopped(failure)
        if failure.held { saveHeld() }
    }

    private func readHeld(_ account: String) -> [String: JoinFailure]? {
        guard FileManager.default.fileExists(atPath: file.path) else { return nil }
        do {
            let held = try JSONDecoder().decode(Held.self, from: Data(contentsOf: file))
            return held.account == account ? held.hosts : nil
        } catch {
            print("sikemux: could not read the hosts held back from joining: \(error)")
            return nil
        }
    }

    private func saveHeld() {
        guard let account else { return }
        var hosts: [String: JoinFailure] = [:]
        for case (let core, .stopped(let failure)) in states where failure.held { hosts[core] = failure }
        do {
            try JSONEncoder().encode(Held(account: account, hosts: hosts)).write(to: file, options: .atomic)
        } catch {
            print("sikemux: could not keep the hosts held back from joining: \(error)")
        }
    }
}
