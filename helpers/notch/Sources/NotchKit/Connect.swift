import SwiftUI

extension PendingDevice: Identifiable {
    var expires: Date { Date(timeIntervalSince1970: Double(expiresAt) / 1000) }
}

/// A device asking to connect, answered from the island.
struct ConnectCard: View {
    let store: NotchStore
    let device: PendingDevice?

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            if let device {
                VStack(alignment: .leading, spacing: 12) {
                    HStack(spacing: 12) {
                        IconView(icon: Icons.phone, size: 20)
                            .foregroundStyle(Theme.ink)
                            .frame(width: 36, height: 36)
                            .background(RoundedRectangle(cornerRadius: 10).fill(Color.white.opacity(0.08)))
                        VStack(alignment: .leading, spacing: 2) {
                            Text("\(device.name.isEmpty ? "A device" : device.name) wants to connect")
                                .font(Theme.ui(14, .semibold))
                                .foregroundStyle(Theme.ink)
                                .lineLimit(1)
                            HStack(spacing: 5) {
                                Text("From your Sikemux account").lineLimit(1)
                                Text("·").foregroundStyle(Theme.inkFaint)
                                TimeLeft(expires: device.expires)
                            }
                            .font(Theme.ui(12))
                            .foregroundStyle(Theme.inkDim)
                        }
                    }
                    HStack(spacing: 8) {
                        CapsuleButton(title: "Decline") { store.answer(device, access: nil) }
                        CapsuleButton(title: "Watch only") { store.answer(device, access: .watch) }
                        CapsuleButton(title: "Allow", primary: true) { store.answer(device, access: .full) }
                    }
                    if let error = store.error {
                        Text(error).font(Theme.ui(11.5)).foregroundStyle(Theme.danger)
                    }
                }
                .padding(14)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(RoundedRectangle(cornerRadius: 16).fill(Theme.raised))
            }
        }
        .padding(.top, 8)
    }
}

/// The time left to answer a device, as m:ss.
private struct TimeLeft: View {
    let expires: Date

    var body: some View {
        TimelineView(.periodic(from: .now, by: 1)) { context in
            Text(Self.format(expires, context.date)).monospacedDigit()
        }
    }

    static func format(_ expires: Date, _ now: Date) -> String {
        let left = max(0, Int(expires.timeIntervalSince(now).rounded(.up)))
        return String(format: "%d:%02d", left / 60, left % 60)
    }
}
