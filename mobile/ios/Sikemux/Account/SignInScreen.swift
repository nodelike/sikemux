import SwiftUI

struct SignInScreen: View {
    @Environment(\.palette) private var palette
    @Environment(\.dismiss) private var dismiss
    @State private var flow = EmailSignIn()
    @State private var step = SignInStep.email
    @State private var email = ""
    @State private var password = ""
    @State private var code = ""
    @State private var busy = false
    @State private var problem: String?

    var body: some View {
        Screen {
            Nav(back: "Back", onBack: back, title: { EmptyView() }, end: { EmptyView() })
            Prompt(icon: Icons.logo, title: title, detail: detail) {
                if case .code(let purpose) = step {
                    CodeEntry(code: $code, failed: problem != nil && code.isEmpty, disabled: busy)
                        .padding(.top, 24)
                        .onChange(of: code) { _, typed in
                            if typed.count == 6 { submitCode(purpose, typed) }
                        }
                    if let problem { ProblemLine(text: problem, centred: true) }
                }
            }
            if !isCode { form }
            Spacer(minLength: 0)
        }
    }

    private var form: some View {
        VStack(spacing: 12) {
            if step == .email {
                Field(placeholder: "you@example.com", text: $email, icon: Icons.inbox, autoFocus: true)
                    .keyboardType(.emailAddress)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .textContentType(.emailAddress)
                    .submitLabel(.next)
                    .onSubmit(submit)
            } else {
                PasswordField(placeholder: "Password", text: $password, autoFocus: true)
                    .textContentType(step == .password ? .password : .newPassword)
                    .submitLabel(.go)
                    .onSubmit(submit)
                    .id(stepKey)
            }
            if let problem { ProblemLine(text: problem) }
            ActionButton(buttonTitle, kind: .primary, busy: busy, action: submit)
                .disabled(step == .email ? email.trimmingCharacters(in: .whitespaces).isEmpty : password.isEmpty)
            if step == .password {
                Button { run { try await flow.forgot() } } label: {
                    Text("Forgot your password?")
                        .font(Theme.ui(15, .medium))
                        .foregroundStyle(palette.secondary)
                        .frame(maxWidth: .infinity, minHeight: 44)
                        .contentShape(Rectangle())
                }
                .buttonStyle(PressFade())
                .disabled(busy)
            }
        }
        .padding(.horizontal, 24)
        .padding(.top, 24)
    }

    private var isCode: Bool {
        if case .code = step { return true }
        return false
    }

    private var stepKey: String {
        switch step {
        case .password: "password"
        case .create: "create"
        default: "new-password"
        }
    }

    private var title: String {
        switch step {
        case .email: "Continue with email"
        case .password: "Enter your password"
        case .create: "Choose a password"
        case .code: "Check your email"
        case .newPassword: "Choose a new password"
        }
    }

    private var detail: String? {
        switch step {
        case .email: nil
        case .code: "Sikemux sent a code to \(email)."
        case .create: "At least 15 characters."
        default: email
        }
    }

    private var buttonTitle: String {
        switch step {
        case .email: "Continue"
        case .password: "Sign in"
        case .create: "Create account"
        default: "Save password"
        }
    }

    private func submit() {
        guard !busy else { return }
        switch step {
        case .email:
            guard !email.trimmingCharacters(in: .whitespaces).isEmpty else { return }
            run { try await flow.submitEmail(email) }
        case .password: run { try await flow.submitPassword(password) }
        case .create: run { try await flow.submitNewAccount(email: email, password: password) }
        case .newPassword: run { try await flow.submitNewPassword(password) }
        case .code: break
        }
    }

    private func submitCode(_ purpose: CodePurpose, _ typed: String) {
        run(clearingCode: true) { try await flow.submitCode(purpose, typed) }
    }

    /// Runs one step, moving on to what it says comes next, or showing its problem instead.
    private func run(clearingCode: Bool = false, _ work: @escaping () async throws -> SignInStep?) {
        busy = true
        problem = nil
        Task {
            defer {
                busy = false
                if clearingCode { code = "" }
            }
            do {
                if let next = try await work() { go(next) } else { Router.shared.root() }
            } catch {
                problem = Sikemux.problem(error)
            }
        }
    }

    private func go(_ next: SignInStep) {
        problem = nil
        password = ""
        code = ""
        step = next
    }

    private func back() {
        if step == .email { return dismiss() }
        flow.reset()
        go(.email)
    }
}
