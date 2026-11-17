import SwiftUI
import UIKit

extension View {
    /// Says when a finger comes down on this view and when the last one lifts, leaving the touch to the scroll view
    /// and buttons it lands on.
    func onTouch(_ touching: @escaping (Bool) -> Void) -> some View {
        modifier(TouchWatching(touching: touching))
    }
}

private struct TouchWatching: ViewModifier {
    let touching: (Bool) -> Void

    func body(content: Content) -> some View {
        if #available(iOS 18, *) {
            // From iOS 18 a SwiftUI drag inside a scroll view stops it scrolling, so UIKit watches the finger.
            content.gesture(TouchRecognizer(touching: touching))
        } else {
            content.simultaneousGesture(
                DragGesture(minimumDistance: 0)
                    .onChanged { _ in touching(true) }
                    .onEnded { _ in touching(false) })
        }
    }
}

@available(iOS 18, *)
private struct TouchRecognizer: UIGestureRecognizerRepresentable {
    let touching: (Bool) -> Void

    func makeCoordinator(converter: CoordinateSpaceConverter) -> Coordinator { Coordinator() }

    func makeUIGestureRecognizer(context: Context) -> UILongPressGestureRecognizer {
        let recognizer = UILongPressGestureRecognizer()
        recognizer.minimumPressDuration = 0
        recognizer.allowableMovement = .greatestFiniteMagnitude
        recognizer.cancelsTouchesInView = false
        recognizer.delaysTouchesEnded = false
        recognizer.delegate = context.coordinator
        return recognizer
    }

    func handleUIGestureRecognizerAction(_ recognizer: UILongPressGestureRecognizer, context: Context) {
        switch recognizer.state {
        case .began: touching(true)
        case .ended, .cancelled, .failed: touching(false)
        default: break
        }
    }

    final class Coordinator: NSObject, UIGestureRecognizerDelegate {
        func gestureRecognizer(
            _ gestureRecognizer: UIGestureRecognizer,
            shouldRecognizeSimultaneouslyWith otherGestureRecognizer: UIGestureRecognizer
        ) -> Bool { true }
    }
}
