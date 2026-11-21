import SwiftUI

/// A screen's ground under its content, edge to edge, with the content kept inside the safe area at the top. A host's
/// screens draw its pane backdrop over the ground.
struct Screen<Content: View>: View {
    @Environment(\.palette) private var palette
    @Environment(\.hostBackdrop) private var backdrop
    @ViewBuilder var content: Content

    var body: some View {
        VStack(spacing: 0) { content }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
            .background(alignment: .top) {
                ZStack(alignment: .top) {
                    palette.ground
                    if let backdrop, backdrop.texture {
                        PaneBackdropView(image: backdrop.image, look: backdrop.look ?? .standard)
                    }
                }
                .ignoresSafeArea()
            }
            .toolbar(.hidden, for: .navigationBar)
    }
}

/// The bar across a screen's top: back, a centred title, and what the screen puts at the end.
struct Nav<Title: View, End: View>: View {
    @Environment(\.palette) private var palette
    @Environment(\.dismiss) private var dismiss
    var back: String?
    var onBack: (() -> Void)?
    @ViewBuilder var title: Title
    @ViewBuilder var end: End

    var body: some View {
        HStack(spacing: 0) {
            Button {
                Haptics.tap()
                if let onBack { onBack() } else { dismiss() }
            } label: {
                HStack(spacing: 2) {
                    IconView(icon: Icons.chevron, size: 18)
                        .rotationEffect(.degrees(180))
                    if let back {
                        Text(LocalizedStringKey(back)).font(Theme.ui(16)).foregroundStyle(palette.secondary)
                    }
                }
                .foregroundStyle(palette.secondary)
                .padding(.leading, 4)
                .padding(.trailing, 8)
                .frame(minWidth: 44, minHeight: 44, alignment: .leading)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel(LocalizedStringKey(back ?? "Back"))

            HStack(spacing: 8) { title }
                .frame(maxWidth: .infinity)

            HStack(spacing: 0) { end }
                .frame(minWidth: End.self == EmptyView.self ? 76 : nil, alignment: .trailing)
        }
        .padding(.horizontal, 6)
        .frame(minHeight: 46)
    }
}

extension Nav where Title == NavTitle, End == EmptyView {
    init(_ title: String, back: String? = nil, onBack: (() -> Void)? = nil) {
        self.init(back: back, onBack: onBack, title: { NavTitle(text: title) }, end: { EmptyView() })
    }
}

extension Nav where Title == NavTitle {
    init(_ title: String, back: String? = nil, onBack: (() -> Void)? = nil, @ViewBuilder end: () -> End) {
        self.init(back: back, onBack: onBack, title: { NavTitle(text: title) }, end: end)
    }
}

struct NavTitle: View {
    @Environment(\.palette) private var palette
    let text: String

    var body: some View {
        Text(LocalizedStringKey(text))
            .font(Theme.ui(16, .semibold))
            .tracking(-0.25)
            .foregroundStyle(palette.ink)
            .lineLimit(1)
    }
}

/// Space under a screen's last content: the home bar, then a little air.
let bottomGap: CGFloat = 12

@MainActor
enum Keyboard {
    static func dismiss() {
        UIApplication.shared.sendAction(#selector(UIResponder.resignFirstResponder), to: nil, from: nil, for: nil)
    }
}

extension View {
    /// A tap anywhere in the scroll view around this content, outside a text field, puts the keyboard away.
    func tapPutsKeyboardAway() -> some View { background(KeyboardTapCatcher()) }
}

private struct KeyboardTapCatcher: UIViewRepresentable {
    func makeUIView(context: Context) -> CatcherView { CatcherView() }
    func updateUIView(_ view: CatcherView, context: Context) {}

    final class CatcherView: UIView, UIGestureRecognizerDelegate {
        private weak var scroll: UIScrollView?
        private lazy var tap: UITapGestureRecognizer = {
            let tap = UITapGestureRecognizer(target: self, action: #selector(tapped))
            tap.cancelsTouchesInView = false
            tap.delegate = self
            return tap
        }()

        override func didMoveToWindow() {
            super.didMoveToWindow()
            scroll?.removeGestureRecognizer(tap)
            var view = superview
            while let current = view, !(current is UIScrollView) { view = current.superview }
            scroll = view as? UIScrollView
            scroll?.addGestureRecognizer(tap)
        }

        @objc private func tapped() { Keyboard.dismiss() }

        func gestureRecognizer(_ recognizer: UIGestureRecognizer, shouldReceive touch: UITouch) -> Bool {
            var view = touch.view
            while let current = view {
                if current is UITextInput { return false }
                view = current.superview
            }
            return true
        }

        func gestureRecognizer(
            _ recognizer: UIGestureRecognizer, shouldRecognizeSimultaneouslyWith other: UIGestureRecognizer
        ) -> Bool { true }
    }
}
