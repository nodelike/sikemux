import Foundation

/// Why an emailed code is asked for: a device new to the account, a new account, or a new password.
enum CodePurpose {
    case trust, verify, reset
}

/// Where signing in with an email address has got to.
enum SignInStep: Equatable {
    case email, password, create
    case code(CodePurpose)
    case newPassword
}

/// Signing in with an email address and a password or emailed code, as the web app does: each step's Clerk calls,
/// throwing Clerk's problem when one refuses, and the step to show next.
@MainActor
final class EmailSignIn {
    private var signIn: Clerk.SignIn?
    private var signUp: Clerk.SignUp?

    private static let unsupportedSignIn = String(
        localized: "This account needs a sign-in step the app does not do yet; sign in on the web first.")
    private static let unsupportedSignUp = String(
        localized: "The account needs more than this app asks for; finish signing up on the web.")

    func reset() {
        signIn = nil
        signUp = nil
    }

    func submitEmail(_ email: String) async throws -> SignInStep {
        do {
            signIn = try await Clerk.call("POST", "v1/client/sign_ins", ["identifier": email.trimmed]).0
            return .password
        } catch let problem as Clerk.Problem where problem.code == "form_identifier_not_found" {
            return .create
        }
    }

    func submitPassword(_ password: String) async throws -> SignInStep? {
        try await step(signIn: "attempt_first_factor", ["strategy": "password", "password": password])
        return try await afterPassword()
    }

    func submitNewAccount(email: String, password: String) async throws -> SignInStep {
        let (made, _) = try await Clerk.call(
            "POST", "v1/client/sign_ups", ["email_address": email.trimmed, "password": password], as: Clerk.SignUp.self
        )
        signUp = made
        signUp = try await Clerk.call(
            "POST", "v1/client/sign_ups/\(made.id)/prepare_verification", ["strategy": "email_code"]
        ).0
        return .code(.verify)
    }

    func forgot() async throws -> SignInStep {
        var form = ["strategy": "reset_password_email_code"]
        form["email_address_id"] =
            signIn?.supportedFirstFactors?.first { $0.strategy == "reset_password_email_code" }?.emailAddressId
        try await step(signIn: "prepare_first_factor", form)
        return .code(.reset)
    }

    func submitNewPassword(_ password: String) async throws -> SignInStep? {
        try await step(signIn: "reset_password", ["password": password])
        return try await afterPassword()
    }

    /// The step after a code, or nil once the person is signed in.
    func submitCode(_ purpose: CodePurpose, _ code: String) async throws -> SignInStep? {
        switch purpose {
        case .trust:
            let client = try await step(signIn: "attempt_second_factor", ["strategy": "email_code", "code": code])
            try await finish(signIn?.createdSessionId, client)
            return nil
        case .reset:
            try await step(signIn: "attempt_first_factor", ["strategy": "reset_password_email_code", "code": code])
            return .newPassword
        case .verify:
            guard let id = signUp?.id else { throw Clerk.Problem(code: nil, message: Self.unsupportedSignUp) }
            let (done, client) = try await Clerk.call(
                "POST", "v1/client/sign_ups/\(id)/attempt_verification", ["strategy": "email_code", "code": code],
                as: Clerk.SignUp.self
            )
            signUp = done
            guard done.status == .complete else { throw Clerk.Problem(code: nil, message: Self.unsupportedSignUp) }
            try await finish(done.createdSessionId, client)
            return nil
        }
    }

    /// After a password, a device new to the account confirms with an emailed code.
    private func afterPassword() async throws -> SignInStep? {
        guard let signIn else { throw Clerk.Problem(code: nil, message: Self.unsupportedSignIn) }
        switch signIn.status {
        case .complete:
            try await finish(signIn.createdSessionId, nil)
            return nil
        case .needsClientTrust, .needsSecondFactor:
            var form = ["strategy": "email_code"]
            form["email_address_id"] =
                signIn.supportedSecondFactors?.first { $0.strategy == "email_code" }?.emailAddressId
            try await step(signIn: "prepare_second_factor", form)
            return .code(.trust)
        default:
            throw Clerk.Problem(code: nil, message: Self.unsupportedSignIn)
        }
    }

    @discardableResult
    private func step(signIn action: String, _ form: [String: String]) async throws -> Clerk.Client? {
        guard let id = signIn?.id else { throw Clerk.Problem(code: nil, message: Self.unsupportedSignIn) }
        let (next, client) = try await Clerk.call(
            "POST", "v1/client/sign_ins/\(id)/\(action)", form, as: Clerk.SignIn.self)
        signIn = next
        return client
    }

    private func finish(_ session: String?, _ client: Clerk.Client?) async throws {
        guard let session else { throw Clerk.Problem(code: nil, message: Self.unsupportedSignIn) }
        try await Account.shared.activate(session: session, client: client)
    }
}

extension String {
    fileprivate var trimmed: String { trimmingCharacters(in: .whitespacesAndNewlines) }
}
