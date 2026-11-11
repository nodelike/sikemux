import SwiftUI

/// Connects to a host on the account; someone at the host still allows it. `arrived` marks a host that just signed in.
struct JoinScreen: View {
    @Environment(\.palette) private var palette
    let core: String
    let name: String
    var arrived = false

    @State private var attempt = 0
    @State private var step = Joining.Step.asking
    @State private var failed: JoinFailure?
    @State private var deadline: Date?

    var body: some View {
        TimelineView(.periodic(from: .now, by: 1)) { context in
            let left = secondsLeft(context.date)
            let problem = failed ?? (left == 0 ? Joining.expired(name) : nil)
            Screen {
                Nav(back: problem == nil ? "Cancel" : "Back", title: { EmptyView() }, end: { EmptyView() })
                if arrived, problem == nil {
                    found
                } else {
                    Prompt(
                        icon: Icons.laptop, tone: problem == nil ? .neutral : .danger,
                        title: problem?.title ?? "Allow this phone on \(name)", detail: problem?.detail
                    )
                    .frame(maxHeight: .infinity, alignment: .top)
                }
                VStack(spacing: 8) {
                    if problem != nil {
                        ActionButton("Try again", kind: .primary, action: retry)
                    } else {
                        waiting(step == .waiting, left)
                    }
                }
                .padding(.horizontal, 16)
                .padding(.top, 12)
                .padding(.bottom, bottomGap)
            }
        }
        .task(id: attempt) { await join() }
        .onAppear { Joining.screens += 1 }
        .onDisappear { Joining.screens -= 1 }
    }

    private var found: some View {
        VStack(spacing: 0) {
            Handshake()
            Text("\(name) just signed in")
                .font(Theme.ui(25, .semibold)).tracking(-0.75).foregroundStyle(palette.ink)
                .multilineTextAlignment(.center).padding(.top, 28)
            let allow = Text("Allow").font(Theme.ui(15, .semibold)).foregroundStyle(palette.ink)
            Text("Click \(allow) on \(name) to let this phone in.")
                .font(Theme.body).lineSpacing(7).foregroundStyle(palette.secondary)
                .multilineTextAlignment(.center).padding(.top, 10)
        }
        .padding(.horizontal, 30)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private func waiting(_ waiting: Bool, _ left: Int) -> some View {
        HStack(spacing: 12) {
            Working()
            Text(waiting ? "Waiting for \(name)" : "Reaching \(name)")
                .font(Theme.ui(15)).foregroundStyle(palette.secondary).lineLimit(1)
                .frame(maxWidth: .infinity, alignment: .leading)
            if waiting {
                Text(String(format: "%d:%02d", left / 60, left % 60)).font(Theme.monoText).foregroundStyle(
                    palette.tertiary)
            }
        }
        .padding(.horizontal, 16)
        .frame(minHeight: 52)
        .background(RoundedRectangle(cornerRadius: Radius.row, style: .continuous).fill(palette.raised))
        .overlay(
            RoundedRectangle(cornerRadius: Radius.row, style: .continuous).strokeBorder(palette.border, lineWidth: 1))
    }

    private func secondsLeft(_ now: Date) -> Int {
        guard step == .waiting, let deadline else { return WireWaits.approvalTimeoutSeconds }
        return max(0, Int(deadline.timeIntervalSince(now).rounded(.up)))
    }

    private func join() async {
        do {
            try await Joining.join(core: core, name: name) { next in
                step = next
                if next == .waiting {
                    deadline = Date().addingTimeInterval(TimeInterval(WireWaits.approvalTimeoutSeconds))
                }
            }
            Haptics.success()
            Router.shared.replace(.device(core: core))
            Notify.shared.offer()
        } catch is CancellationError {
        } catch {
            guard !Task.isCancelled else { return }
            Haptics.failure()
            failed = error as? JoinFailure ?? JoinFailure(title: "Connecting stopped", detail: Sikemux.problem(error))
        }
    }

    private func retry() {
        failed = nil
        deadline = nil
        step = .asking
        attempt += 1
    }
}

/// The phone and the host with a dot travelling the line between them.
private struct Handshake: View {
    @Environment(\.palette) private var palette
    @Environment(\.accessibilityReduceMotion) private var still
    private static let line: CGFloat = 84
    private static let dot: CGFloat = 6

    var body: some View {
        HStack(spacing: 6) {
            end(Icons.phone)
            ZStack(alignment: .leading) {
                HStack(spacing: 0) {
                    ForEach(0..<Int(Self.line / Self.dot), id: \.self) { _ in
                        Rectangle().fill(palette.rest).frame(width: Self.dot / 2, height: 2)
                            .frame(width: Self.dot, alignment: .leading)
                    }
                }
                TimelineView(.animation(paused: still)) { context in
                    let t =
                        still
                        ? 0 : context.date.timeIntervalSinceReferenceDate.truncatingRemainder(dividingBy: 1.6) / 1.6
                    let eased = t < 0.5 ? 2 * t * t : 1 - pow(-2 * t + 2, 2) / 2
                    Circle().fill(palette.live)
                        .frame(width: Self.dot, height: Self.dot)
                        .shadow(color: palette.live, radius: 5)
                        .offset(x: eased * (Self.line - Self.dot))
                }
            }
            .frame(width: Self.line, height: Self.dot)
            end(Icons.laptop)
        }
        .accessibilityHidden(true)
    }

    private func end(_ icon: IconDef) -> some View {
        let shape = RoundedRectangle(cornerRadius: Radius.card, style: .continuous)
        return IconView(icon: icon, size: 30)
            .foregroundStyle(palette.ink)
            .frame(width: 64, height: 64)
            .background(shape.fill(palette.raised))
            .overlay(shape.strokeBorder(palette.borderStrong, lineWidth: 1))
    }
}
