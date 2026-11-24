import Foundation

// Verifies the real iOS `raceTimeout` helper against an un-cancellable async operation (modeling UniFFI 0.31.2
// `connection.host()` and `device.close()`, which ignore Swift task cancellation so `withTaskGroup` in 7a1d8168f
// waited out the full 30s request timeout / 3s close timeout instead of returning in 1s / 200ms).

@main
enum ReconnectCheck {
    static func main() async {
        // 1. Un-cancellable work that completes in 5s must time out at 100ms, not 5s.
        let started = ContinuousClock.now
        let timedOut: Bool? = await raceTimeout(for: .milliseconds(100)) {
            await withCheckedContinuation { continuation in
                DispatchQueue.global().asyncAfter(deadline: .now() + .seconds(5)) {
                    continuation.resume(returning: true)
                }
            }
        }
        let elapsed = started.duration(to: .now)
        guard timedOut == nil else {
            fputs("expected timeout nil, got \(String(describing: timedOut))\n", stderr)
            exit(1)
        }
        guard elapsed < .seconds(1) else {
            fputs("raceTimeout waited \(elapsed) instead of returning near 100ms\n", stderr)
            exit(1)
        }

        // 2. Fast work must return its value before the deadline.
        let fast: Int? = await raceTimeout(for: .seconds(2)) {
            try? await Task.sleep(for: .milliseconds(10))
            return 42
        }
        guard fast == 42 else {
            fputs("expected 42, got \(String(describing: fast))\n", stderr)
            exit(1)
        }

        print("ok")
    }
}
