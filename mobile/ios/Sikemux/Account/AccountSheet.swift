import SwiftUI
import UIKit

/// The signed-in account's picture, or its initials on a neutral circle when it has none.
struct Avatar: View {
    @Environment(\.palette) private var palette
    @Environment(\.displayScale) private var scale
    let size: CGFloat

    var body: some View {
        let user = Account.shared.user
        ZStack {
            Circle().fill(palette.raised)
            Text(initials(user))
                .font(Theme.ui((size * 0.4).rounded(), .semibold))
                .tracking(-0.2)
                .foregroundStyle(palette.secondary)
            if let url = picture(user) {
                AsyncImage(url: url) { image in
                    image.resizable().scaledToFill()
                } placeholder: {
                    Color.clear
                }
            }
        }
        .frame(width: size, height: size)
        .clipShape(Circle())
        .overlay(Circle().strokeBorder(palette.border, lineWidth: 1))
    }

    private func picture(_ user: Clerk.User?) -> URL? {
        guard let user, user.hasImage == true, let url = user.imageUrl else { return nil }
        let pixels = Int(size * scale)
        return URL(string: "\(url)\(url.contains("?") ? "&" : "?")width=\(pixels)&height=\(pixels)&fit=scale-down&quality=100")
    }

    private func initials(_ user: Clerk.User?) -> String {
        let words = (user?.name ?? "").split(whereSeparator: \.isWhitespace)
        let ends = [words.first, words.count > 1 ? words.last : nil].compactMap { $0?.first.map(String.init) }.joined()
        return (ends.isEmpty ? (user?.email?.first.map(String.init) ?? "") : ends).uppercased()
    }
}

extension View {
    /// The account: who is signed in, this phone, the lock, signing out and deleting the account.
    func accountSheet(shown: Binding<Bool>) -> some View {
        sheet(shown: shown) { AccountSheet(shown: shown) }
    }
}

struct AccountSheet: View {
    @Environment(\.palette) private var palette
    @Binding var shown: Bool
    @State private var leaving = false
    @State private var unconfirmed: String?
    private let account = Account.shared
    private let session = AccountSession.shared

    var body: some View {
        VStack(spacing: 0) {
            head
            phone
            LockRow()
            if let unconfirmed {
                Text(unconfirmed)
                    .font(Theme.ui(15, .semibold)).foregroundStyle(palette.ink)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 8).padding(.top, 12)
                Text("This phone stays on your account until removed at app.sikemux.com.")
                    .font(Theme.meta).lineSpacing(5).foregroundStyle(palette.tertiary)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 8).padding(.top, 4).padding(.bottom, 12)
                VStack(spacing: 8) {
                    ActionButton("Try again") { leave() }.disabled(leaving)
                    ActionButton(leaving ? "Signing out…" : "Sign out anyway", kind: .danger, icon: Icons.logOut) { leave(anyway: true) }
                        .disabled(leaving)
                }
            } else {
                ActionButton(leaving ? "Signing out…" : "Sign out", kind: .danger, icon: Icons.logOut) { leave() }
                    .disabled(leaving)
                    .padding(.top, 16)
            }
            Button {
                shown = false
                Router.shared.push(.deleteAccount)
            } label: {
                Text("Delete account…")
                    .font(Theme.ui(15, .medium)).foregroundStyle(palette.secondary)
                    .frame(maxWidth: .infinity, minHeight: 44)
                    .contentShape(Rectangle())
            }
            .buttonStyle(PressFade())
            .disabled(leaving)
            .padding(.top, 4)
            Text("Sikemux \(Self.version)")
                .font(Theme.ui(12)).foregroundStyle(palette.tertiary)
                .padding(.top, 14)
        }
        .onChange(of: shown) { _, now in if !now { unconfirmed = nil } }
    }

    private var head: some View {
        let user = account.user
        return HStack(spacing: 12) {
            Avatar(size: 44)
            VStack(alignment: .leading, spacing: 0) {
                Text(user?.name ?? user?.email ?? "Signed in")
                    .font(Theme.ui(16, .semibold)).tracking(-0.25).foregroundStyle(palette.ink).lineLimit(1)
                if user?.name != nil, let email = user?.email {
                    Text(email).metaStyle(palette).lineLimit(1)
                }
                Text("Signed in with \(user?.providerName ?? "email")").metaStyle(palette)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(.horizontal, 8)
        .padding(.top, 6)
        .padding(.bottom, 14)
    }

    private var phone: some View {
        let id = try? Identity.shared.deviceIdentity().id()
        return HStack(spacing: 12) {
            IconView(icon: Icons.phone, size: 18).foregroundStyle(palette.secondary)
            VStack(alignment: .leading, spacing: 0) {
                Text(UIDevice.current.name).font(Theme.ui(15, .medium)).tracking(-0.15).foregroundStyle(palette.ink)
                (Text("This phone") + Text(id.map { " · \($0.prefix(8))" } ?? "").font(Theme.mono(12)))
                    .metaStyle(palette)
                if case .failed(let problem, _) = session.status {
                    Text("Not on your account yet: \(problem)").font(Theme.meta).foregroundStyle(palette.danger)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            if case .failed = session.status {
                Button("Try again") { session.retryNow() }
                    .font(Theme.ui(15, .medium))
                    .foregroundStyle(palette.accent)
                    .buttonStyle(PressFade())
            }
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 6)
        .frame(minHeight: 52)
        .background(RoundedRectangle(cornerRadius: 9, style: .continuous).fill(palette.raised))
        .overlay(RoundedRectangle(cornerRadius: 9, style: .continuous).strokeBorder(palette.border, lineWidth: 1))
    }

    /// Takes the phone off the account first, so hosts hear of it; when that fails, asks before leaving it there.
    private func leave(anyway: Bool = false) {
        leaving = true
        unconfirmed = nil
        Task {
            defer { leaving = false }
            if anyway {
                await Leave.signOutHere(confirmed: false)
            } else {
                do {
                    try await Leave.signOut()
                } catch {
                    let problem = error as? ApiProblem
                    unconfirmed = problem.map { $0.unreachable ? "Can't reach Sikemux" : $0.message } ?? "Can't reach Sikemux"
                    return
                }
            }
            shown = false
        }
    }

    /// The installed release and the commit it was built from: `0.1.0-nightly.5 · cef3bae`.
    static let version: String = {
        let installed = Versions.fromBuild(Config.build) ?? Config.version
        let commit = Bundle.main.object(forInfoDictionaryKey: "SikemuxCommit") as? String ?? ""
        return commit.isEmpty ? installed : "\(installed) · \(commit)"
    }()
}

/// Says why the phone signed out, before Welcome.
struct FarewellScreen: View {
    let farewell: Farewell

    var body: some View {
        let restore = "Sign in again within \(Config.restorableDays) days to restore it."
        let (icon, title, detail): (IconDef, String, String?) = switch farewell {
        case .removed: (Icons.phone, "This phone was removed from your account", nil)
        case .deleted: (Icons.trash, "Your account was deleted", restore)
        case .deletedHere: (Icons.trash, "Your account is deleted", restore)
        }
        PromptScreen(icon: icon, title: title, detail: detail) {
            ActionButton("Continue", kind: .primary) { AccountSession.shared.farewell = nil }
        }
    }
}

/// Signed in to an account that was deleted but can still be restored.
struct RestoreAccountScreen: View {
    @Environment(\.palette) private var palette
    let deletesAt: String
    @State private var busy = false
    @State private var problem: String?

    var body: some View {
        Screen {
            VStack(spacing: 0) {
                PromptGlyph(icon: Icons.hourglass)
                Text("Your account is being deleted")
                    .font(Theme.ui(22, .semibold)).tracking(-0.7).foregroundStyle(palette.ink)
                    .multilineTextAlignment(.center).padding(.top, 20)
                (Text("It will be deleted for good on ") + Text(day).foregroundStyle(palette.ink) + Text("."))
                    .font(Theme.body).lineSpacing(7).foregroundStyle(palette.secondary)
                    .multilineTextAlignment(.center).padding(.top, 8)
                if let problem { ProblemLine(text: problem, centred: true).padding(.top, 8) }
            }
            .padding(.horizontal, 28)
            .padding(.bottom, 120)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            VStack(spacing: 8) {
                ActionButton("Restore account", kind: .primary, busy: busy, action: restore).disabled(busy)
                ActionButton("Sign out", icon: Icons.logOut) { Task { await Leave.signOutHere(confirmed: true) } }
                    .disabled(busy)
            }
            .padding(.horizontal, 16).padding(.top, 12).padding(.bottom, bottomGap)
        }
    }

    private var day: String {
        Format.date(iso: deletesAt)?.formatted(.dateTime.day().month(.wide).year()) ?? deletesAt
    }

    private func restore() {
        busy = true
        problem = nil
        Task {
            do {
                _ = try await Account.shared.token(fresh: true)
                _ = try await AccountCalls.restore()
                AccountSession.shared.retryNow()
            } catch {
                busy = false
                problem = Sikemux.problem(error)
            }
        }
    }
}

/// The only screen while the installed app is older than the accounts server works with.
struct UpdateRequiredScreen: View {
    let required: Network.UpdateRequired

    var body: some View {
        PromptScreen(
            icon: Icons.download,
            title: "Update Sikemux",
            detail: "This version, \(required.current), is too old. Install \(required.minimum) or later."
        ) {
            ActionButton("Get the update", kind: .primary) { UIApplication.shared.open(Config.updateLink) }
        }
    }
}
