import Foundation
import os

/// Races `work` against `duration` without waiting for `work` to finish after timeout, since UniFFI async
/// calls do not observe Swift task cancellation.
func raceTimeout<T: Sendable>(for duration: Duration, _ work: @escaping @Sendable () async -> T) async -> T? {
    await withCheckedContinuation { continuation in
        let done = OSAllocatedUnfairLock(initialState: false)
        let finish: @Sendable (T?) -> Void = { value in
            let first = done.withLock { already -> Bool in
                if already { return false }
                already = true
                return true
            }
            if first { continuation.resume(returning: value) }
        }
        Task { finish(await work()) }
        Task {
            try? await Task.sleep(for: duration)
            finish(nil)
        }
    }
}
