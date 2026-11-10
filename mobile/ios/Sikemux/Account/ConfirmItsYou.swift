import SwiftUI

/// The person backed out of proving it is them; nothing was done.
struct ReverifyCancelled: Error {}

/// Runs requests the server takes only after a sign-in from the last ten minutes. When it asks for one, `verify` holds
/// what the person must prove, and the request carries on by itself once they have. `elsewhere` says where else to do
/// it, for an account the app can't verify.
@MainActor @Observable
final class RecentSignIn {
    enum Factor: Equatable {
        case password
        case emailCode(to: String, emailAddressId: String)
    }

    struct Verify: Equatable {
        let factor: Factor
        var entry = ""
        var checking = false
        var problem: String?
    }

    private(set) var verify: Verify?
    @ObservationIgnored private let elsewhere: String
    @ObservationIgnored private var waiting: CheckedContinuation<Void, Error>?

    init(elsewhere: String) {
        self.elsewhere = elsewhere
    }

    /// Runs `work`, asking the person to prove it is them when the server wants it. Throws ReverifyCancelled when they
    /// back out.
    func run<T>(_ work: () async throws -> T) async throws -> T {
        do {
            return try await work()
        } catch let problem as ApiProblem where problem.reverifyWithinMinutes != nil {
            try await ask()
            _ = try await Account.shared.token(fresh: true)
            return try await work()
        }
    }

    func enter(_ entry: String) {
        guard verify?.checking == false else { return }
        verify?.entry = entry
        verify?.problem = nil
    }

    func cancel() {
        verify = nil
        waiting?.resume(throwing: ReverifyCancelled())
        waiting = nil
    }

    func check() async {
        guard let asked = verify, let session = Account.shared.sessionId else { return }
        verify?.checking = true
        verify?.problem = nil
        let form: [String: String] = switch asked.factor {
        case .password: ["strategy": "password", "password": asked.entry]
        case .emailCode: ["strategy": "email_code", "code": asked.entry]
        }
        do {
            let (result, _) = try await Clerk.call(
                "POST", "v1/client/sessions/\(session)/verify/attempt_first_factor", form, as: Clerk.SessionVerification.self
            )
            guard result.status == .complete else {
                return giveUp("This account also needs a second step to confirm it. \(elsewhere)")
            }
        } catch {
            verify?.checking = false
            verify?.entry = ""
            verify?.problem = Sikemux.problem(error)
            return
        }
        verify = nil
        waiting?.resume()
        waiting = nil
    }

    private func ask() async throws {
        guard let session = Account.shared.sessionId else { throw Clerk.Problem(code: nil, message: "Sign in again.") }
        let (started, _) = try await Clerk.call(
            "POST", "v1/client/sessions/\(session)/verify", ["level": "first_factor"], as: Clerk.SessionVerification.self
        )
        let factors = started.supportedFirstFactors ?? []
        let factor: Factor
        if factors.contains(where: { $0.strategy == "password" }) {
            factor = .password
        } else if let email = factors.first(where: { $0.strategy == "email_code" && $0.emailAddressId != nil }) {
            factor = .emailCode(to: email.safeIdentifier ?? "your email", emailAddressId: email.emailAddressId!)
            _ = try await Clerk.call(
                "POST", "v1/client/sessions/\(session)/verify/prepare_first_factor",
                ["strategy": "email_code", "email_address_id": email.emailAddressId!], as: Clerk.SessionVerification.self
            )
        } else {
            throw Clerk.Problem(code: nil, message: "This account can't confirm it's you in the app. \(elsewhere)")
        }
        try await withCheckedThrowingContinuation { continuation in
            waiting = continuation
            verify = Verify(factor: factor)
        }
    }

    private func giveUp(_ problem: String) {
        verify = nil
        waiting?.resume(throwing: Clerk.Problem(code: nil, message: problem))
        waiting = nil
    }
}

/// Asks for the password, or the code sent to the account's email, whose sixth digit sends it.
struct ConfirmItsYou: View {
    @Environment(\.palette) private var palette
    let recent: RecentSignIn
    let verify: RecentSignIn.Verify
    /// The button under the password, named for what it goes on to do.
    let action: String
    var kind: ButtonKind = .primary
    var icon: IconDef?

    var body: some View {
        Screen {
            Nav(back: "Back", onBack: recent.cancel, title: { EmptyView() }, end: { EmptyView() })
            if case .emailCode(let to, _) = verify.factor {
                Prompt(icon: Icons.shield, title: "Confirm it's you", detail: "Sikemux sent a code to \(to).") {
                    CodeEntry(
                        code: Binding(get: { verify.entry }, set: { typed in
                            recent.enter(typed)
                            if typed.count == 6 { Task { await recent.check() } }
                        }),
                        failed: verify.problem != nil && verify.entry.isEmpty,
                        disabled: verify.checking
                    )
                    .padding(.top, 24)
                    if let problem = verify.problem { ProblemLine(text: problem, centred: true).padding(.top, 8) }
                }
            } else {
                Prompt(icon: Icons.shield, title: "Confirm it's you")
                VStack(spacing: 12) {
                    PasswordField(placeholder: "Password", text: Binding(get: { verify.entry }, set: recent.enter), autoFocus: true)
                        .textContentType(.password)
                        .submitLabel(.go)
                        .onSubmit { Task { await recent.check() } }
                    ActionButton(verify.checking ? "Checking…" : action, kind: kind, icon: icon) { Task { await recent.check() } }
                        .disabled(verify.checking || verify.entry.isEmpty)
                    if let problem = verify.problem { ProblemLine(text: problem) }
                }
                .padding(.horizontal, 24)
                .padding(.top, 24)
            }
            Spacer(minLength: 0)
        }
    }
}
