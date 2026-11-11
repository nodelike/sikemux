import SwiftUI

/// The first screen, signed out: what the app is for, and the ways to sign in. Drawn in the default theme, since no
/// host is paired yet.
struct WelcomeScreen: View {
    @Environment(\.palette) private var palette
    @State private var busy: ProviderSignIn.Provider?
    @State private var problem: String?

    var body: some View {
        VStack(spacing: 0) {
            Reel()
            VStack(alignment: .leading, spacing: 0) {
                Spacer(minLength: 0)
                HStack(spacing: 9) {
                    IconView(icon: Icons.logo, size: 22).foregroundStyle(palette.ink)
                    Text("Sikemux").font(Theme.ui(15, .semibold)).tracking(-0.2).foregroundStyle(palette.ink)
                }
                Text("Your agents,\non your phone.")
                    .font(Theme.ui(34, .semibold))
                    .tracking(-1.2)
                    .lineSpacing(-4)
                    .foregroundStyle(palette.ink)
                    .padding(.top, 18)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottomLeading)
            .padding(.horizontal, 24)

            VStack(spacing: 12) {
                provider(.apple, "Continue with Apple", BrandMarks.apple)
                provider(.google, "Continue with Google", BrandMarks.google)
                provider(.github, "Continue with GitHub", BrandMarks.github)
                NavigationLink(value: Route.signIn) {
                    Text("Continue with email")
                        .font(Theme.ui(15, .medium))
                        .foregroundStyle(palette.secondary)
                        .frame(maxWidth: .infinity, minHeight: 44)
                        .contentShape(Rectangle())
                }
                .buttonStyle(PressFade())
                if let problem { ProblemLine(text: problem, centred: true) }
            }
            .padding(.horizontal, 16)
            .padding(.top, 28)
            .padding(.bottom, bottomGap)
        }
        .background(palette.ground.ignoresSafeArea())
        .toolbar(.hidden, for: .navigationBar)
    }

    private func provider(_ provider: ProviderSignIn.Provider, _ title: String, _ mark: IconDef) -> some View {
        ActionButton(title: title, busy: busy == provider, action: { start(provider) }) {
            IconView(icon: mark, size: 18)
        }
    }

    private func start(_ provider: ProviderSignIn.Provider) {
        guard busy == nil else { return }
        busy = provider
        problem = nil
        Task {
            defer { busy = nil }
            do {
                try await ProviderSignIn.start(provider)
            } catch is ProviderSignIn.Cancelled {
            } catch {
                problem = Sikemux.problem(error)
            }
        }
    }
}

/// The rail, drifting past: what the app is for, before there is anything of the person's to show.
private struct Reel: View {
    @Environment(\.palette) private var palette
    @Environment(\.accessibilityReduceMotion) private var still

    private struct Line {
        let provider: String
        let title: String
        let place: String
        let doing: String
        let state: State
    }

    private enum State {
        case working, needs, done
    }

    private static let lines = [
        Line(
            provider: "claude", title: "Tighten the rail spacing", place: "sikemux", doing: "Editing…", state: .working),
        Line(
            provider: "codex", title: "Fix the flaky PTY replay test", place: "sikemux", doing: "Running a command…",
            state: .working),
        Line(provider: "claude", title: "Remote pairing screen", place: "sikemux", doing: "Needs input", state: .needs),
        Line(
            provider: "hermes", title: "Release notes for v0.4.3", place: "sikemux-front", doing: "Done", state: .done),
        Line(
            provider: "opencode", title: "Why does the shader dim the rail", place: "sikemux", doing: "Thinking…",
            state: .working),
        Line(
            provider: "codex", title: "Download size on the hero", place: "sikemux-front", doing: "Reading…",
            state: .working),
    ]
    private static let rowHeight: CGFloat = 56
    private static let period: TimeInterval = 26

    var body: some View {
        TimelineView(.animation(paused: still)) { context in
            let progress =
                still
                ? 0
                : context.date.timeIntervalSinceReferenceDate.truncatingRemainder(dividingBy: Self.period) / Self.period
            VStack(spacing: 0) {
                ForEach(0..<(Self.lines.count * 2), id: \.self) { index in
                    row(Self.lines[index % Self.lines.count])
                }
            }
            .offset(y: -progress * Self.rowHeight * CGFloat(Self.lines.count))
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        }
        .padding(.horizontal, 16)
        .frame(maxHeight: 340)
        .clipped()
        .overlay(alignment: .top) { fade(top: true) }
        .overlay(alignment: .bottom) { fade(top: false) }
        .padding(.top, 8)
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }

    private func row(_ line: Line) -> some View {
        HStack(spacing: 12) {
            AgentMark(provider: line.provider, size: 22)
            VStack(alignment: .leading, spacing: 1) {
                Text(line.title).font(Theme.row).tracking(-0.15).foregroundStyle(palette.ink).lineLimit(1)
                HStack(spacing: 4) {
                    IconView(icon: Icons.folders, size: 11).foregroundStyle(palette.live)
                    Text("\(line.place) · \(line.doing)").font(Theme.ui(12.5)).foregroundStyle(palette.tertiary)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            Group {
                switch line.state {
                case .working: Working()
                case .needs: NeedsYou()
                case .done: Dot(color: palette.accent)
                }
            }
            .frame(width: 20)
        }
        .padding(.horizontal, 10)
        .frame(height: Self.rowHeight)
    }

    private func fade(top: Bool) -> some View {
        LinearGradient(
            colors: top ? [palette.ground, palette.ground.opacity(0)] : [palette.ground.opacity(0), palette.ground],
            startPoint: .top, endPoint: .bottom
        )
        .frame(height: 110)
    }
}
