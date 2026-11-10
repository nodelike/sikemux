import SwiftUI

/// What shows until the person passes Face ID, Touch ID or the passcode. It asks by itself as it opens.
struct LockScreen: View {
    private let lock = Lock.shared

    var body: some View {
        PromptScreen(icon: Icons.lock, title: "Sikemux is locked") {
            ActionButton("Unlock", kind: .primary, busy: lock.asking) { Task { await lock.unlock() } }
        }
        .task { await lock.unlock() }
    }
}

/// Covers the app and any sheet over it while it is away, so the app switcher's picture shows nothing, and shows
/// the lock once it has been away a while.
struct LockCover: View {
    @Environment(\.palette) private var palette
    private let lock = Lock.shared

    var body: some View {
        if lock.phase != .open {
            ZStack {
                palette.ground.ignoresSafeArea()
                if lock.phase == .locked { LockScreen() }
            }
        }
    }
}

/// The account sheet's switch for asking for Face ID, Touch ID or the passcode each time the app opens.
struct LockRow: View {
    @Environment(\.palette) private var palette
    private let lock = Lock.shared
    @State private var problem: String?

    var body: some View {
        HStack(spacing: 12) {
            IconView(icon: Icons.lock, size: 18).foregroundStyle(palette.secondary)
            VStack(alignment: .leading, spacing: 0) {
                Text("Lock Sikemux").font(Theme.ui(15, .medium)).tracking(-0.15).foregroundStyle(palette.ink)
                if let problem { Text(problem).metaStyle(palette) }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            Toggle("Lock Sikemux", isOn: Binding(get: { lock.on }, set: flip))
                .labelsHidden()
                .tint(palette.live)
                .disabled(lock.asking)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 6)
        .frame(minHeight: 52)
        .background(RoundedRectangle(cornerRadius: 9, style: .continuous).fill(palette.raised))
        .overlay(RoundedRectangle(cornerRadius: 9, style: .continuous).strokeBorder(palette.border, lineWidth: 1))
        .padding(.top, 8)
    }

    private func flip(_ next: Bool) {
        problem = nil
        Task {
            if next { problem = await lock.turnOn() } else { await lock.turnOff() }
        }
    }
}
