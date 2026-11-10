import SwiftUI

/// A host waiting behind a sign-in code, to check before approving it.
struct PendingHost: Decodable {
    let key: String
    let name: String
    let platform: String
    let channel: String
    let requestedAt: String
    let requestedFrom: String?
    let requestedNear: String?
    let removedAt: String?
}

/// Signs in a host with no browser: the person types the code `sikemux login` shows there, checks when and where the
/// host asked from, and adds it after proving it is them. A link never fills the code in.
struct HostSignInScreen: View {
    private enum Step {
        case enter
        case check(code: String, host: PendingHost)
        case done(name: String, added: Bool)
    }

    private enum Busy {
        case look, add, refuse
    }

    @Environment(\.palette) private var palette
    @Environment(\.dismiss) private var dismiss
    @State private var recent = RecentSignIn(elsewhere: "Add it at app.sikemux.com/host.")
    @State private var typed = ""
    @State private var step = Step.enter
    @State private var busy: Busy?
    @State private var problem: String?

    var body: some View {
        if let verify = recent.verify {
            ConfirmItsYou(recent: recent, verify: verify, action: "Add host")
        } else {
            switch step {
            case .enter: enter
            case .check(let code, let host): check(code, host)
            case .done(let name, let added): done(name, added)
            }
        }
    }

    private var enter: some View {
        let entered = typed.trimmingCharacters(in: .whitespaces)
        return Screen {
            Nav(back: "Devices", title: { EmptyView() }, end: { EmptyView() })
            ScrollView(showsIndicators: false) {
                VStack(spacing: 0) {
                    VStack(spacing: 0) {
                        PromptGlyph(icon: Icons.server)
                        Text("Add a host with a code")
                            .font(Theme.ui(22, .semibold)).tracking(-0.7).foregroundStyle(palette.ink)
                            .padding(.top, 20)
                        (Text("Enter the code ") + Text("sikemux login").font(Theme.monoText).foregroundStyle(palette.ink)
                            + Text(" shows on the computer."))
                            .font(Theme.body).lineSpacing(7).foregroundStyle(palette.secondary)
                            .multilineTextAlignment(.center).padding(.top, 8)
                    }
                    .padding(.horizontal, 28)
                    .padding(.top, 32)
                    VStack(spacing: 12) {
                        Field(placeholder: "BCDF-GHJK", text: $typed, icon: Icons.terminal, autoFocus: true, font: Theme.mono(20))
                            .tracking(2)
                            .textInputAutocapitalization(.characters)
                            .autocorrectionDisabled()
                            .submitLabel(.go)
                            .onSubmit { if !entered.isEmpty { look(entered) } }
                            .accessibilityLabel("Code")
                        if let problem { ProblemLine(text: problem) }
                        ActionButton("Continue", kind: .primary, busy: busy == .look) { look(entered) }
                            .disabled(entered.isEmpty || busy != nil)
                    }
                    .padding(.horizontal, 24)
                    .padding(.top, 24)
                }
                .padding(.bottom, bottomGap)
            }
            .scrollDismissesKeyboard(.never)
        }
    }

    private func check(_ code: String, _ host: PendingHost) -> some View {
        let near = host.requestedNear ?? host.requestedFrom
        let platform = ["ios": "iOS", "android": "Android", "macos": "macOS", "linux": "Linux"][host.platform] ?? host.platform
        return Screen {
            Nav(back: "Back", onBack: { step = .enter }, title: { EmptyView() }, end: { EmptyView() })
            ScrollView(showsIndicators: false) {
                Prompt(
                    icon: host.platform == "linux" ? Icons.desktop : Icons.laptop,
                    title: "Add \(host.name) to your account?",
                    detail: "\(platform) · \(host.channel). Only add a computer you are setting up right now."
                )
                VStack(spacing: 12) {
                    Rows {
                        Row(title: asked(host.requestedAt)) {
                            IconView(icon: Icons.clock, size: 18).foregroundStyle(palette.secondary)
                        } end: { EmptyView() }
                        Row(
                            title: near.map { "From \($0)" } ?? "From an unknown place",
                            detail: host.requestedNear != nil ? host.requestedFrom : nil
                        ) {
                            IconView(icon: Icons.globe, size: 18).foregroundStyle(palette.secondary)
                        } end: { EmptyView() }
                    }
                    if let removed = host.removedAt.flatMap(Format.date(iso:)) {
                        Text("This host was removed from your account on \(removed.formatted(.dateTime.day().month(.wide).year())). Add it back only if you are setting it up again yourself.")
                            .font(Theme.body).foregroundStyle(palette.danger)
                    }
                    if let problem { ProblemLine(text: problem) }
                    ActionButton("Add host", kind: .primary, busy: busy == .add) { add(code, host) }
                        .disabled(busy != nil)
                    ActionButton("Not mine", kind: .danger, busy: busy == .refuse) {
                        run(.refuse) {
                            try await AccountAPI.send("DELETE", "/v1/host-sign-ins/\(code)")
                            return .done(name: host.name, added: false)
                        }
                    }
                    .disabled(busy != nil)
                }
                .padding(.horizontal, 24)
                .padding(.top, 24)
                .padding(.bottom, bottomGap)
            }
        }
    }

    private func done(_ name: String, _ added: Bool) -> some View {
        Screen {
            Nav(back: "Devices", title: { EmptyView() }, end: { EmptyView() })
            Prompt(
                icon: added ? Icons.check : Icons.close,
                title: added ? "\(name) is on your account" : "Refused",
                detail: added ? nil : "\(name) was not added."
            )
            ActionButton("Done") { dismiss() }
                .padding(.horizontal, 24)
                .padding(.top, 24)
            Spacer(minLength: 0)
        }
    }

    private func asked(_ at: String) -> String {
        let ago = Format.ago(Format.date(iso: at))
        return "Asked \(ago == "now" ? "just now" : ago)"
    }

    private func look(_ code: String) {
        run(.look) {
            let host = try await AccountAPI.send("GET", "/v1/host-sign-ins/\(code)", as: PendingHost.self)
            return .check(code: code, host: host)
        }
    }

    private func add(_ code: String, _ host: PendingHost) {
        run(.add) {
            try await recent.run {
                _ = try await Account.shared.token(fresh: true)
                try await AccountAPI.send("POST", "/v1/host-sign-ins/\(code)/approve")
            }
            AccountHosts.shared.read()
            return .done(name: host.name, added: true)
        }
    }

    private func run(_ action: Busy, _ work: @escaping () async throws -> Step) {
        busy = action
        problem = nil
        Task {
            defer { busy = nil }
            do {
                step = try await work()
                Haptics.success()
            } catch is ReverifyCancelled {
            } catch {
                Haptics.failure()
                problem = Sikemux.problem(error)
            }
        }
    }
}
