import SwiftUI
import UIKit

/// The field a secret is typed into: its words never show, and the keyboard neither suggests nor remembers them,
/// which SwiftUI's text field cannot promise.
struct SecretField: UIViewRepresentable {
    @Binding var text: String
    @Binding var focused: Bool
    let accent: UIColor
    let dark: Bool

    func makeUIView(context: Context) -> UITextView {
        let field = UITextView()
        field.delegate = context.coordinator
        field.backgroundColor = .clear
        field.textColor = .clear
        field.font = UIFont(name: "Figtree-Regular", size: 15.5) ?? .systemFont(ofSize: 15.5)
        field.textContainerInset = UIEdgeInsets(top: 10, left: 0, bottom: 10, right: 0)
        field.textContainer.lineFragmentPadding = 0
        field.takesSecret(true)
        field.textContentType = nil
        return field
    }

    func updateUIView(_ field: UITextView, context: Context) {
        context.coordinator.parent = self
        if field.text != text { field.text = text }
        // A caret after hidden characters would float mid-field; the size under the field shows the typing.
        field.tintColor = text.isEmpty ? accent : .clear
        field.keyboardAppearance = dark ? .dark : .light
    }

    func makeCoordinator() -> Coordinator { Coordinator(parent: self) }

    final class Coordinator: NSObject, UITextViewDelegate {
        var parent: SecretField
        init(parent: SecretField) { self.parent = parent }

        func textViewDidChange(_ field: UITextView) { parent.text = field.text }
        func textViewDidBeginEditing(_ field: UITextView) { parent.focused = true }
        func textViewDidEndEditing(_ field: UITextView) { parent.focused = false }
    }
}

extension UITextView {
    /// Has the keyboard correct, suggest and remember nothing typed here while `secret`, and help as usual otherwise.
    func takesSecret(_ secret: Bool) {
        let corrects: UITextAutocorrectionType = secret ? .no : .default
        guard autocorrectionType != corrects else { return }
        autocorrectionType = corrects
        spellCheckingType = secret ? .no : .default
        autocapitalizationType = secret ? .none : .sentences
        smartQuotesType = secret ? .no : .default
        smartDashesType = secret ? .no : .default
        smartInsertDeleteType = secret ? .no : .default
        inlinePredictionType = secret ? .no : .default
        if isFirstResponder { reloadInputViews() }
    }
}
