import Observation

/// Something just done from this phone, shown with a way to take it back for a few seconds, or until something
/// else takes its place.
@MainActor @Observable
final class Offer<Value> {
    private let shownFor: Duration = .seconds(6)
    private(set) var current: Value?
    @ObservationIgnored private var hiding: Task<Void, Never>?

    func offer(_ value: Value) {
        hold(value)
        hiding = Task {
            try? await Task.sleep(for: shownFor)
            if !Task.isCancelled { current = nil }
        }
    }

    /// Shows `value` until something replaces or clears it.
    func hold(_ value: Value) {
        hiding?.cancel()
        current = value
    }

    func clear() {
        hiding?.cancel()
        current = nil
    }
}
