import SwiftUI

/// The working mark: a small spinner.
struct Working: View {
    var body: some View { ProgressView().controlSize(.mini) }
}

extension ChatSession {
    /// Puts waiting messages into the running turn now, as one, instead of after it.
    func steer(_ ids: [String]) {}
    func dropQueued(_ id: String) {}
}

struct MenuItem: View {
    @Environment(\.palette) private var palette
    let icon: IconDef
    let label: String
    var danger = false
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 12) {
                IconView(icon: icon, size: 18)
                Text(label).font(Theme.ui(15.5, .medium))
                Spacer(minLength: 0)
            }
            .foregroundStyle(danger ? palette.danger : palette.ink)
            .padding(.horizontal, 14)
            .frame(minHeight: 52)
            .background(RoundedRectangle(cornerRadius: 14).fill(palette.raised))
            .overlay(RoundedRectangle(cornerRadius: 14).strokeBorder(palette.border, lineWidth: 1))
        }
        .buttonStyle(.plain)
    }
}
