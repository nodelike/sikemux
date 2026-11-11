import SwiftUI
import UIKit

/// Copies `value`, and says so with a tick for a moment.
struct CopyButton: View {
    @Environment(\.palette) private var palette
    let value: String
    let label: String
    var size: CGFloat = 13
    /// Says Copy beside the icon, and Copied once it has copied, as a code block's pill does.
    var worded = false
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
            if worded {
                HStack(spacing: 6) {
                    IconView(icon: copied ? Icons.check : Icons.copy, size: size)
                    Text(copied ? "Copied" : "Copy").font(Theme.ui(11.5, .medium))
                }
                .foregroundStyle(copied ? palette.ink : palette.secondary)
                .padding(.horizontal, 9)
                .frame(height: 26)
                .overlay(RoundedRectangle(cornerRadius: 7).strokeBorder(palette.border, lineWidth: 1))
                .contentShape(Rectangle())
            } else {
                IconView(icon: copied ? Icons.check : Icons.copy, size: size)
                    .foregroundStyle(copied ? palette.ink : palette.inkFaint)
                    .frame(width: 26, height: 26)
                    .contentShape(Rectangle())
            }
        }
        .buttonStyle(ActiveFill(radius: 7, rest: worded ? palette.overlay : .clear))
        .accessibilityLabel(copied ? "Copied" : "Copy \(label)")
    }
}

/// A pressed control fills with the palette's active colour, as the Mac's do on hover.
struct ActiveFill: ButtonStyle {
    @Environment(\.palette) private var palette
    var radius: CGFloat = 6
    /// The fill while it is not pressed.
    var rest: Color = .clear

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .background(RoundedRectangle(cornerRadius: radius).fill(configuration.isPressed ? palette.active : rest))
    }
}
