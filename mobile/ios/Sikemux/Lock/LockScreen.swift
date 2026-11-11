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
        SwitchRow(
            icon: Icons.lock, title: "Lock Sikemux", isOn: Binding(get: { lock.on }, set: { flip($0) }),
            disabled: lock.asking
        ) {
            if let problem { Text(problem).metaStyle(palette) }
        }
    }

    private func flip(_ next: Bool) {
        problem = nil
        Task {
            if next { problem = await lock.turnOn() } else { await lock.turnOff() }
        }
    }
}
