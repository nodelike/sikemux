import SwiftUI

/// Six code tiles over a hidden number field, so the system keyboard fills them one by one. The field stays
/// editable while a code is checked, so the keyboard never drops; typing then is ignored.
struct CodeEntry: View {
    @Environment(\.palette) private var palette
    @Binding var code: String
    var failed = false
    var disabled = false
    @FocusState private var focused: Bool

    private static let length = 6

    var body: some View {
        ZStack {
            TextField("", text: Binding(
                get: { code },
                set: { typed in
                    if !disabled { code = String(typed.filter(\.isNumber).prefix(Self.length)) }
                }
            ))
            .keyboardType(.numberPad)
            .textContentType(.oneTimeCode)
            .focused($focused)
            .opacity(0.02)
            .frame(width: 1, height: 1)
            .accessibilityHidden(true)

            HStack(spacing: 8) {
                ForEach(0..<Self.length, id: \.self) { index in
                    tile(index)
                        .padding(.leading, index == Self.length / 2 ? 6 : 0)
                }
            }
        }
        .contentShape(Rectangle())
        .onTapGesture { focused = true }
        .onAppear { focused = true }
        .onChange(of: failed) { _, now in
            if now {
                Haptics.failure()
                focused = true
            }
        }
        .accessibilityElement()
        .accessibilityLabel(failed ? "Code, wrong" : "Code")
        .accessibilityValue(code.isEmpty ? "Empty" : "\(code.map(String.init).joined(separator: " ")), \(code.count) of \(Self.length) digits")
        .accessibilityHint("Type the \(Self.length) digits")
    }

    private func tile(_ index: Int) -> some View {
        let digits = Array(code)
        let shape = RoundedRectangle(cornerRadius: 10, style: .continuous)
        let current = !failed && !disabled && index == digits.count
        return Text(index < digits.count ? String(digits[index]) : "")
            .font(Theme.mono(26))
            .foregroundStyle(failed ? palette.danger : palette.ink)
            .frame(width: 46, height: 58)
            .background(shape.fill(failed ? palette.danger.opacity(0.08) : disabled ? palette.raised : palette.sunken))
            .overlay(
                shape.strokeBorder(
                    failed ? palette.danger.opacity(0.55)
                        : disabled ? palette.borderStrong
                        : current ? palette.borderSelected : palette.border,
                    lineWidth: 1
                )
            )
    }
}
