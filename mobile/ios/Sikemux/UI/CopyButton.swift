import SwiftUI
import UIKit

/// Copies `value`, and says so with a tick for a moment.
struct CopyButton: View {
    @Environment(\.palette) private var palette
    let value: String
    let label: String
    var size: CGFloat = 13
    @State private var copied = false

    var body: some View {
        Button {
            UIPasteboard.general.string = value
            Haptics.select()
            copied = true
            Task {
                try? await Task.sleep(for: .milliseconds(1200))
                copied = false
            }
        } label: {
            IconView(icon: copied ? Icons.check : Icons.copy, size: size)
                .foregroundStyle(copied ? palette.ink : palette.inkFaint)
                .frame(width: 26, height: 26)
                .contentShape(Rectangle())
        }
        .buttonStyle(ActiveFill(radius: 7))
        .accessibilityLabel(copied ? "Copied" : "Copy \(label)")
    }
}

/// A pressed control fills with the palette's active colour, as the Mac's do on hover.
struct ActiveFill: ButtonStyle {
    @Environment(\.palette) private var palette
    var radius: CGFloat = 6

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .background(RoundedRectangle(cornerRadius: radius).fill(configuration.isPressed ? palette.active : .clear))
    }
}
