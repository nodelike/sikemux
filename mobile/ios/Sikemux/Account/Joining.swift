import Foundation
import SikemuxCore
import UIKit

/// Why joining a host stopped, in words for the person.
struct JoinFailure: Error, Equatable {
    let title: String
    let detail: String
}

/// Joining a host on the account: a ticket from Sikemux, handed to the host, then waiting while someone there decides.
@MainActor
enum Joining {
    enum Step {
        case asking, waiting
    }

    /// How long the host keeps a request open before it stops asking.
    static let approvalSeconds = 120

    /// Join screens showing, so a host arriving meanwhile waits in Devices rather than taking over.
    static var screens = 0

    static func expired(_ host: String) -> JoinFailure {
        JoinFailure(title: "\(host) did not answer", detail: "Nobody allowed this phone within two minutes.")
    }

    /// Joins the account's host `core`; an allowed phone keeps the host as paired.
    static func join(core: String, name: String, step: (Step) -> Void) async throws {
        step(.asking)
        let ticket: JoinTicket
        do {
            ticket = try await AccountCalls.joinTicket(host: core)
        } catch {
            throw ticketFailure(name, error)
        }
        try Task.checkCancellation()
        step(.waiting)
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
        case .denied:
            throw JoinFailure(title: "\(name) said no", detail: "Someone at \(name) turned this phone down.")
        case .refused(let reason):
            throw refusal(name, reason)
        case .allowed(let access):
            await Hub.shared.remember(
                PairedDevice(core: core, access: access, pairedAt: Date().timeIntervalSince1970 * 1000, name: name))
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
