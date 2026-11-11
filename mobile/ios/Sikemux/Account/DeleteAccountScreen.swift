import AuthenticationServices
import SwiftUI

/// Deletes the account, after the person types the word and, when the server asks, proves it is them again.
struct DeleteAccountScreen: View {
    @Environment(\.palette) private var palette
    @State private var recent = RecentSignIn(elsewhere: "Delete it at app.sikemux.com/delete-account.")
    @State private var typed = ""
    @State private var deleting = false
    @State private var problem: String?

    /// The word typed to confirm deleting the account, in the phone app and at app.sikemux.com alike.
    private static let word = "delete"

    private var gone: [String] {
        [
            "Hosts are signed out.",
            "Phones, this one too, are signed out and removed from every host.",
            "After \(Config.restorableDays) days your sign-in, device keys and history are erased, and backups \(Config.backupDays) days later.",
        ]
    }

    private var confirmed: Bool { typed.trimmingCharacters(in: .whitespaces).lowercased() == Self.word }

    var body: some View {
        if let verify = recent.verify {
            ConfirmItsYou(recent: recent, verify: verify, action: "Delete account", kind: .danger, icon: Icons.trash)
        } else {
            Screen {
                Nav(back: "Back", title: { EmptyView() }, end: { EmptyView() })
                ScrollView(showsIndicators: false) {
                    head
                    VStack(alignment: .leading, spacing: 12) {
                        VStack(alignment: .leading, spacing: 8) {
                            ForEach(gone, id: \.self) { line in
                                HStack(alignment: .firstTextBaseline, spacing: 10) {
                                    Text("•").foregroundStyle(palette.tertiary)
                                    Text(line).foregroundStyle(palette.secondary)
                                        .frame(maxWidth: .infinity, alignment: .leading)
                                }
                                .font(Theme.body).lineSpacing(7)
                                .padding(.trailing, 8)
                            }
                        }
                        Text("Type \(Text(Self.word).font(Theme.monoText).foregroundStyle(palette.ink)) to confirm")
                            .metaStyle(palette)
                            .padding(.top, 8)
                            .padding(.bottom, -4)
                        Field(placeholder: Self.word, text: $typed)
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                            .submitLabel(.go)
                            .onSubmit(start)
                            .disabled(deleting)
                        if let problem { ProblemLine(text: problem) }
                        ActionButton(
                            deleting ? "Deleting…" : "Delete account", kind: .danger, icon: Icons.trash, action: start
                        )
                        .disabled(deleting || !confirmed)
                    }
                    .padding(.horizontal, 24)
                    .padding(.top, 24)
                    .padding(.bottom, bottomGap)
                }
            }
        }
    }

    private var head: some View {
        VStack(spacing: 0) {
            PromptGlyph(icon: Icons.trash, tone: .danger)
            Text("Delete your account")
                .font(Theme.ui(22, .semibold)).tracking(-0.7).foregroundStyle(palette.ink)
                .padding(.top, 20)
            let whose = Account.shared.user?.email.map { Text($0).foregroundStyle(palette.ink) } ?? Text("your account")
            Text("This deletes \(whose). You can restore it within \(Config.restorableDays) days.")
                .font(Theme.body).lineSpacing(7).foregroundStyle(palette.secondary)
                .multilineTextAlignment(.center).padding(.top, 8)
        }
        .padding(.horizontal, 28)
        .padding(.top, 32)
    }

    private func start() {
        guard confirmed, !deleting else { return }
        deleting = true
        problem = nil
        Task {
            do {
                let apple = try await appleConsent()
                try await recent.run {
                    _ = try await Account.shared.token(fresh: true)
                    try await AccountCalls.delete(appleAuthorizationCode: apple)
                }
            } catch is ReverifyCancelled {
                deleting = false
                return
            } catch is ProviderSignIn.Cancelled {
                deleting = false
                return
            } catch {
                deleting = false
                problem = Sikemux.problem(error)
                return
            }
            await Leave.signOutHere(farewell: .deletedHere, confirmed: true)
        }
    }

    /// Before an account that signs in with Apple is deleted, Apple's sheet gives a fresh code, which the server
    /// revokes so Apple forgets the app too. Closing the sheet cancels; when Apple can't give a code, the deletion goes
    /// on without one.
    private func appleConsent() async throws -> String? {
        guard Account.shared.user?.signsInWithApple == true else { return nil }
        do {
            let credential = try await AppleSheet.ask(scopes: [])
            return credential.authorizationCode.flatMap { String(data: $0, encoding: .utf8) }
        } catch is ProviderSignIn.Cancelled {
            throw ProviderSignIn.Cancelled()
        } catch {
            return nil
        }
    }
}
