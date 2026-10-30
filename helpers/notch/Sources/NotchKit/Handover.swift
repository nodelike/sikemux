import Foundation

/// One helper per build: a newer one replaces the one already running, and a
/// stable or nightly helper steps aside while Sikemux Dev's helper runs.
enum Handover {
    private static var held: Int32 = -1

    static func lockPath(stateDir: String, dev: Bool) -> String {
        (stateDir as NSString).appendingPathComponent(dev ? "notch.dev.lock" : "notch.lock")
    }

    /// Takes this build's lock, ending the helper that held it first.
    static func claim(stateDir: String, dev: Bool) -> Bool {
        try? FileManager.default.createDirectory(atPath: stateDir, withIntermediateDirectories: true)
        let path = lockPath(stateDir: stateDir, dev: dev)
        let descriptor = open(path, O_RDWR | O_CREAT | O_CLOEXEC, 0o600)
        guard descriptor >= 0 else { return false }
        if flock(descriptor, LOCK_EX | LOCK_NB) != 0 {
            let holder = (try? String(contentsOfFile: path, encoding: .utf8))
                .flatMap { pid_t($0.trimmingCharacters(in: .whitespacesAndNewlines)) }
                .flatMap { $0 > 0 && $0 != getpid() ? $0 : nil }
            // Helpers before this one could ignore SIGTERM, so one that outlives it is killed.
            for signal in [SIGTERM, SIGKILL] {
                if let holder { kill(holder, signal) }
                if wait(for: descriptor, seconds: 1.5) { break }
            }
            guard flock(descriptor, LOCK_EX | LOCK_NB) == 0 else {
                close(descriptor)
                return false
            }
        }
        ftruncate(descriptor, 0)
        let pid = "\(getpid())\n"
        _ = pid.withCString { write(descriptor, $0, strlen($0)) }
        held = descriptor
        return true
    }

    private static func wait(for descriptor: Int32, seconds: TimeInterval) -> Bool {
        let deadline = Date().addingTimeInterval(seconds)
        while flock(descriptor, LOCK_EX | LOCK_NB) != 0 {
            guard Date() < deadline else { return false }
            usleep(50_000)
        }
        return true
    }

    /// Whether Sikemux Dev's helper is running now.
    static func devIsRunning(stateDir: String) -> Bool {
        let descriptor = open(lockPath(stateDir: stateDir, dev: true), O_RDONLY | O_CLOEXEC)
        guard descriptor >= 0 else { return false }
        defer { close(descriptor) }
        if flock(descriptor, LOCK_SH | LOCK_NB) == 0 {
            flock(descriptor, LOCK_UN)
            return false
        }
        return errno == EWOULDBLOCK
    }
}
