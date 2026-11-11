import SwiftUI
import UIKit

/// Lets a screen hand the keyboard to the composer, or take it away.
@MainActor
final class ComposerFocus: ObservableObject {
    fileprivate weak var field: UITextView?
    /// The prompts this chat sent before are on show, to pick one again.
    @Published var recent = false

    func request() { field?.becomeFirstResponder() }
    /// Empties the field, words the keyboard is still composing too.
    func clear() { field?.text = "" }
}

/// The chat composer's text field: grows with its text to a few lines, then scrolls; draws the draft's chips apart
/// from the words around them.
struct ComposerInput: UIViewRepresentable {
    @Environment(\.palette) private var palette
    @Binding var text: String
    var tokens: [TextToken] = []
    let placeholder: String
    var editable = true
    var focus: ComposerFocus?
    var onFocus: (Bool) -> Void = { _ in }

    static let minHeight: CGFloat = 44
    static let maxHeight: CGFloat = 160
    private static let size: CGFloat = 14.5
    private static let line: CGFloat = 20

    func makeUIView(context: Context) -> UITextView {
        let field = PlaceholderTextView()
        field.delegate = context.coordinator
        field.backgroundColor = .clear
        field.textContainerInset = UIEdgeInsets(top: 9, left: 10, bottom: 4, right: 10)
        field.textContainer.lineFragmentPadding = 0
        field.isScrollEnabled = false
        field.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        field.accessibilityLabel = "Message"
        focus?.field = field
        return field
    }

    func updateUIView(_ field: UITextView, context: Context) {
        context.coordinator.parent = self
        focus?.field = field
        let field = field as! PlaceholderTextView
        field.isEditable = editable
        field.tintColor = UIColor(palette.accent)
        field.keyboardAppearance = palette.isLight ? .light : .dark
        field.typingAttributes = attributes(UIColor(palette.ink))
        field.placeholder = NSAttributedString(string: placeholder, attributes: attributes(UIColor(palette.inkFaint)))
        if field.text != text || context.coordinator.drawnTokens != tokens {
            let selection = field.selectedRange
            let replaced = field.text != text
            field.attributedText = styled()
            context.coordinator.drawnTokens = tokens
            let length = (text as NSString).length
            field.selectedRange = NSRange(location: replaced ? length : min(selection.location, length), length: 0)
        }
        field.setNeedsDisplay()
    }

    func sizeThatFits(_ proposal: ProposedViewSize, uiView field: UITextView, context: Context) -> CGSize? {
        let width = proposal.width ?? field.bounds.width
        guard width > 0 else { return nil }
        let fitted = field.sizeThatFits(CGSize(width: width, height: .greatestFiniteMagnitude)).height
        let placeholderHeight = (field as! PlaceholderTextView).placeholderHeight(width: width)
        let natural = max(Self.minHeight, text.isEmpty ? placeholderHeight : fitted)
        let scrolls = natural > Self.maxHeight
        if field.isScrollEnabled != scrolls { DispatchQueue.main.async { field.isScrollEnabled = scrolls } }
        return CGSize(width: width, height: min(natural, Self.maxHeight))
    }

    func makeCoordinator() -> Coordinator { Coordinator(parent: self) }

    private func attributes(_ color: UIColor) -> [NSAttributedString.Key: Any] {
        let style = NSMutableParagraphStyle()
        style.minimumLineHeight = Self.line
        style.maximumLineHeight = Self.line
        let font = UIFont(name: "Figtree-Regular", size: Self.size) ?? .systemFont(ofSize: Self.size)
        return [
            .font: font, .foregroundColor: color, .paragraphStyle: style,
            .baselineOffset: (Self.line - font.lineHeight) / 4,
        ]
    }

    private func styled() -> NSAttributedString {
        let styled = NSMutableAttributedString(string: text, attributes: attributes(UIColor(palette.ink)))
        let length = styled.length
        for token in tokens where token.start < token.end && token.end <= length {
            styled.addAttributes(
                [.foregroundColor: UIColor(palette.accent), .backgroundColor: UIColor(palette.active)],
                range: NSRange(location: token.start, length: token.end - token.start)
            )
        }
        return styled
    }

    final class Coordinator: NSObject, UITextViewDelegate {
        var parent: ComposerInput
        var drawnTokens: [TextToken] = []

        init(parent: ComposerInput) { self.parent = parent }

        func textViewDidChange(_ field: UITextView) {
            if field.markedTextRange != nil { return }
            parent.text = field.text
            field.setNeedsDisplay()
        }

        func textViewDidBeginEditing(_ field: UITextView) { parent.onFocus(true) }
        func textViewDidEndEditing(_ field: UITextView) {
            if field.text != parent.text { textViewDidChange(field) }
            parent.onFocus(false)
        }
    }
}

/// A text view that draws its placeholder while it is empty, wrapping as the text would.
private final class PlaceholderTextView: UITextView {
    var placeholder: NSAttributedString?

    func placeholderHeight(width: CGFloat) -> CGFloat {
        guard let placeholder else { return 0 }
        let inner = width - textContainerInset.left - textContainerInset.right
        let height = placeholder.boundingRect(
            with: CGSize(width: inner, height: .greatestFiniteMagnitude), options: .usesLineFragmentOrigin, context: nil
        ).height
        return ceil(height) + textContainerInset.top + textContainerInset.bottom
    }

    override func draw(_ rect: CGRect) {
        super.draw(rect)
        guard text.isEmpty, let placeholder else { return }
        let inner = bounds.inset(by: textContainerInset)
        placeholder.draw(with: inner, options: .usesLineFragmentOrigin, context: nil)
    }
}
