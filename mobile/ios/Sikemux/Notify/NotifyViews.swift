import SwiftUI

/// The account sheet's switch for notifications, and the way to the system's settings when it blocks them.
struct NotificationsRow: View {
    @Environment(\.palette) private var palette
    @Environment(\.openURL) private var openURL
    @State private var busy = false
    private let notify = Notify.shared

    var body: some View {
        HStack(spacing: 12) {
            IconView(icon: Icons.inbox, size: 18).foregroundStyle(palette.secondary)
            VStack(alignment: .leading, spacing: 0) {
                Text("Notifications").font(Theme.ui(15, .medium)).foregroundStyle(palette.ink)
                if notify.blocked {
                    Button {
                        if let settings = URL(string: UIApplication.openSettingsURLString) { openURL(settings) }
                    } label: {
                        Text("Allow them in Settings").font(Theme.ui(13, .medium)).foregroundStyle(palette.accent)
                    }
                    .buttonStyle(.plain)
                    .accessibilityAddTraits(.isLink)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            Toggle("Notifications", isOn: Binding(get: { notify.on }, set: { flip($0) }))
                .labelsHidden()
                .tint(palette.live)
                .disabled(busy)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 6)
        .frame(minHeight: 52)
        .background(RoundedRectangle(cornerRadius: 9).fill(palette.raised))
        .overlay(RoundedRectangle(cornerRadius: 9).strokeBorder(palette.border, lineWidth: 1))
        .padding(.top, 8)
        .task { await notify.checkAllowed() }
    }

    private func flip(_ next: Bool) {
        busy = true
        Task {
            if next { await notify.turnOn() } else { await notify.turnOff() }
            busy = false
        }
    }
}

/// Asked once, after the first host lets a signed-in phone in, since notifications reach it through the account.
struct NotificationsOffer: ViewModifier {
    @Environment(\.palette) private var palette
    private let notify = Notify.shared

    func body(content: Content) -> some View {
        @Bindable var notify = notify
        content.sheet(shown: $notify.offered) {
            VStack(spacing: 0) {
                VStack(spacing: 12) {
                    PromptGlyph(icon: Icons.inbox, small: true)
                    Text("Turn on notifications?").font(Theme.ui(17, .semibold)).tracking(-0.35).foregroundStyle(palette.ink)
                }
                .padding(.top, 12)
                .padding(.bottom, 20)
                HStack(spacing: 8) {
                    ActionButton("Not now") { notify.declineOffer() }
                    ActionButton("Turn on", kind: .primary) { Task { await notify.turnOn() } }
                }
            }
        }
    }
}

extension View {
    func notificationsOffer() -> some View { modifier(NotificationsOffer()) }
}
