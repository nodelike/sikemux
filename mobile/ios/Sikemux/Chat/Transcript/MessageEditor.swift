import SwiftUI

/// Writing a sent message again where its bubble was.
struct MessageEditor: View {
    @Environment(\.palette) private var palette
    @Environment(TranscriptContext.self) private var context
    let rowId: String
    let text: String
    @State private var draft: String
    @FocusState private var focused: Bool

    init(rowId: String, text: String) {
        self.rowId = rowId
        self.text = text
        _draft = State(initialValue: text)
    }

    var body: some View {
        let words = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        VStack(alignment: .trailing, spacing: 8) {
            TextField("", text: $draft, axis: .vertical)
                .sentWords(palette)
                .tint(palette.accent)
                .lineLimit(1...12)
                .focused($focused)
                .accessibilityLabel("Edit message")
            HStack(spacing: 6) {
                Button {
                    context.actions.cancelEdit()
                } label: {
                    Text("Cancel").font(Theme.ui(13, .semibold)).foregroundStyle(palette.ink)
                        .padding(.horizontal, 12).frame(height: 30)
                        .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(palette.borderStrong, lineWidth: 1))
                }
                .buttonStyle(ActiveFill(radius: 8))
                Button {
                    context.actions.sendEdit(rowId, words)
                } label: {
                    Text("Send").font(Theme.ui(13, .semibold)).foregroundStyle(palette.sunken)
                        .padding(.horizontal, 12).frame(height: 30)
                        .background(RoundedRectangle(cornerRadius: 8).fill(palette.ink))
                }
                .buttonStyle(PressFade())
                .disabled(words.isEmpty)
                .opacity(words.isEmpty ? 0.6 : 1)
            }
        }
        .padding(.top, 10)
        .padding(.bottom, 8)
        .padding(.horizontal, 14)
        .background(Bubble.mine.fill(palette.bubbleFill))
        .overlay(Bubble.mine.strokeBorder(palette.borderSelected, lineWidth: 1))
        .sentWidth()
        .onAppear { focused = true }
    }
}
