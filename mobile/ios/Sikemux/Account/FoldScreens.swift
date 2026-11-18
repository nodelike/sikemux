import SwiftUI

private let elsewhere = String(localized: "Bring your accounts together at app.sikemux.com.")

/// What the signed-in account sees until the fold gate opens.
struct FoldScreen: View {
    let gate: Fold.Gate

    var body: some View {
        switch gate {
        case .caught(let caught):
            NavigationStack {
                CatchScreen(caught: caught)
                    .navigationDestination(for: Route.self) { ScreenFor(route: $0) }
            }
        case .confirm(let waiting): ConfirmFoldScreen(waiting: waiting)
        case .checking, .open: Checking()
        }
    }
}

private struct Checking: View {
    @Environment(\.palette) private var palette

    var body: some View {
        Screen {
            ProgressView().tint(palette.secondary).frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }
}

/// A prompt whose detail is an address in ink, with the API's words under it.
private struct FoldPrompt<Actions: View>: View {
    @Environment(\.palette) private var palette
    let icon: IconDef
    let title: String
    let address: String?
    let words: String
    let problem: String?
    @ViewBuilder var actions: Actions

    var body: some View {
        Screen {
            Prompt(icon: icon, title: title, fill: true) {
                if let address {
                    Text(address).font(Theme.body).foregroundStyle(palette.ink).padding(.top, 8)
                }
                Text(words)
                    .font(Theme.body).lineSpacing(7).foregroundStyle(palette.secondary)
                    .multilineTextAlignment(.center).padding(.top, 12)
                if let problem { ProblemLine(text: problem, centred: true).padding(.top, 8) }
            }
            VStack(spacing: 8) { actions }
                .padding(.horizontal, 16).padding(.top, 12).padding(.bottom, bottomGap)
        }
    }
}

private struct CatchScreen: View {
    @Environment(\.palette) private var palette
    let caught: FoldCatch
    @State private var recent = RecentSignIn(elsewhere: elsewhere)
    @State private var busy: Busy?
    @State private var problem: String?

    private enum Busy { case start, keep }

    var body: some View {
        if let deletesAt = caught.deletesAt {
            RestoreAccountScreen(deletesAt: deletesAt, onRestored: { Fold.shared.askAgain() })
        } else if let verify = recent.verify {
            ConfirmItsYou(recent: recent, verify: verify, action: "Use my account")
        } else {
            FoldPrompt(
                icon: caught.mustFold ? Icons.shield : Icons.user, title: caught.title ?? "", address: caught.address,
                words: caught.words ?? "", problem: problem
            ) {
                ActionButton("Use my account", kind: .primary, busy: busy == .start, action: start)
                if caught.mustFold {
                    ActionButton("Sign out", icon: Icons.logOut) { Task { await Leave.signOutHere(confirmed: true) } }
                    NavigationLink(value: Route.deleteAccount) {
                        Text("Delete this account instead").font(Theme.meta).foregroundStyle(palette.secondary).frame(maxWidth: .infinity, minHeight: 44)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(PressFade())
                } else {
                    ActionButton("I'm new here", busy: busy == .keep, action: keepNew)
                }
            }
            .disabled(busy != nil)
            .toolbar(.hidden, for: .navigationBar)
        }
    }

    private func start() {
        let why = caught.why ?? .browser
        busy = .start
        problem = nil
        Task {
            do {
                let intent = try await recent.run { try await AccountCalls.startFold(why: why) }
                Fold.shared.hold(intent, why: why)
                await Leave.signOutHere(confirmed: true)
            } catch {
                busy = nil
                if !(error is ReverifyCancelled) { problem = Sikemux.problem(error) }
            }
        }
    }

    private func keepNew() {
        busy = .keep
        problem = nil
        Task {
            do {
                try await AccountCalls.keepNewAccount()
                Fold.shared.open()
            } catch {
                busy = nil
                problem = Sikemux.problem(error)
            }
        }
    }
}

/// Above the sign-in buttons, while a fold waits for the person's other account.
struct FoldWaiting: View {
    @Environment(\.palette) private var palette
    let fold = Fold.shared

    var body: some View {
        if let waiting = fold.waiting {
            VStack(spacing: 8) {
                Text(waiting.words)
                    .font(Theme.body)
                    .foregroundStyle(palette.secondary)
                    .multilineTextAlignment(.center)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.horizontal, 8)
                Button("Cancel") { fold.drop() }.font(Theme.meta).foregroundStyle(palette.secondary).buttonStyle(PressFade())
            }
            .padding(.bottom, 8)
        }
    }
}

private struct ConfirmFoldScreen: View {
    let waiting: Fold.Waiting
    @State private var recent = RecentSignIn(elsewhere: elsewhere)
    @State private var read = Read.loading
    @State private var busy = false
    @State private var problem: String?

    private enum Read {
        case loading
        case failed(problem: String, sameAccount: Bool)
        case loaded(FoldPreview)
        case done(title: String, words: String)
    }

    private var seatComing: Bool { waiting.why == .seat }

    var body: some View {
        content.task(id: waiting) { await load() }
    }

    @ViewBuilder
    private var content: some View {
        if let verify = recent.verify {
            ConfirmItsYou(recent: recent, verify: verify, action: seatComing ? "Move the seat" : "Bring it here")
        } else {
            switch read {
            case .loading: Checking()
            case .failed(let problem, let sameAccount):
                PromptScreen(icon: Icons.circleAlert, title: "Nothing was brought over", detail: problem) {
                    if sameAccount {
                        ActionButton("Sign in with another account", kind: .primary, action: signInAgain)
                        ActionButton("Cancel", action: cancel)
                    } else {
                        ActionButton("Continue", kind: .primary, action: cancel)
                    }
                }
            case .done(let title, let words):
                PromptScreen(icon: Icons.circleCheck, title: title, detail: words) {
                    ActionButton(seatComing ? "Sign in" : "Continue", kind: .primary) {
                        if seatComing { signInAgain() } else { Fold.shared.open() }
                    }
                }
            case .loaded(let preview):
                FoldPrompt(
                    icon: seatComing ? Icons.user : Icons.shield, title: preview.title, address: preview.address,
                    words: preview.words, problem: problem
                ) {
                    ActionButton(seatComing ? "Move the seat" : "Bring it here", kind: .primary, busy: busy) {
                        bring(preview)
                    }
                    ActionButton("Cancel", action: cancel)
                }
                .disabled(busy)
            }
        }
    }

    private func load() async {
        do {
            read = .loaded(try await AccountCalls.previewFold(waiting))
        } catch {
            let problem = error as? ApiProblem
            if problem?.status == 404 { Fold.shared.drop() }
            read = .failed(problem: Sikemux.problem(error), sameAccount: problem?.code == .sameAccount)
        }
    }

    private func bring(_ preview: FoldPreview) {
        busy = true
        problem = nil
        Task {
            do {
                try await recent.run { try await AccountCalls.confirmFold(waiting) }
                Fold.shared.drop()
                read = .done(title: preview.doneTitle, words: preview.doneWords)
            } catch {
                busy = false
                if !(error is ReverifyCancelled) { problem = Sikemux.problem(error) }
            }
        }
    }

    private func cancel() {
        Fold.shared.drop()
        Fold.shared.askAgain()
    }

    private func signInAgain() {
        Task { await Leave.signOutHere(confirmed: true) }
    }
}
