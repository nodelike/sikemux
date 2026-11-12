import SwiftUI

/// A question the agent went on without, on the assumption it stated: Fine lets it stand, Change answers it.
struct AssumedLine: View {
    @Environment(\.palette) private var palette
    let question: AgentQuestion
    let canAnswer: Bool
    let onFine: () -> Void
    let onChange: () -> Void

    var body: some View {
        HStack(alignment: .top, spacing: 7) {
            IconView(icon: Icons.check, size: 13).foregroundStyle(palette.inkDim).frame(height: 18)
            Text(words)
                .font(Theme.ui(12.5))
                .lineSpacing(18 - 12.5 * 1.2)
                .foregroundStyle(palette.inkDim)
                .tint(palette.ink)
                .frame(maxWidth: .infinity, alignment: .leading)
                .environment(
                    \.openURL,
                    OpenURLAction { url in
                        Haptics.select()
                        if url.host() == "fine" { onFine() } else { onChange() }
                        return .handled
                    })
        }
        .padding(.top, 10)
        .padding(.bottom, 6)
    }

    /// The assumption in one run of words, with Fine and Change as taps inside it.
    private var words: AttributedString {
        var name = AttributedString("@\(question.name)")
        name.font = Theme.ui(12.5, .semibold)
        name.foregroundColor = palette.ink
        var words = name + AttributedString(" \(CallWords.assumed(question.assumption ?? ""))")
        guard canAnswer else { return words }
        words += AttributedString("  ·  ")
        for (label, host) in [(CallWords.fine, "fine"), (CallWords.change, "change")] {
            var act = AttributedString(label)
            act.font = Theme.ui(12.5, .medium)
            act.foregroundColor = palette.ink
            act.link = URL(string: "sikemux-act://\(host)")
            words += act
            if host == "fine" { words += AttributedString("   ") }
        }
        return words
    }
}
