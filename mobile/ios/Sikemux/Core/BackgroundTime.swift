import UIKit

/// Keeps the app running a little after it leaves the screen, so work such as joining a host can finish.
@MainActor
enum BackgroundTime {
    /// Runs `work`, asking iOS not to suspend the app until it is done, for as long as iOS allows. When the time
    /// runs out iOS ends it, and the work carries on only if the app is brought back.
    static func run<T>(_ name: String, _ work: () async throws -> T) async rethrows -> T {
        var task = UIBackgroundTaskIdentifier.invalid
        task = UIApplication.shared.beginBackgroundTask(withName: name) {
            UIApplication.shared.endBackgroundTask(task)
            task = .invalid
        }
        defer {
            if task != .invalid { UIApplication.shared.endBackgroundTask(task) }
        }
        return try await work()
    }
}
