import UIKit

/// The few taps the app gives back. A phone without a haptic engine stays still.
@MainActor
enum Haptics {
    private static let light = UIImpactFeedbackGenerator(style: .light)
    private static let selection = UISelectionFeedbackGenerator()
    private static let notification = UINotificationFeedbackGenerator()

    static func tap() { light.impactOccurred() }
    static func select() { selection.selectionChanged() }
    static func success() { notification.notificationOccurred(.success) }
    static func warning() { notification.notificationOccurred(.warning) }
    static func failure() { notification.notificationOccurred(.error) }
}
