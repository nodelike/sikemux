import Foundation

struct Command: Decodable {
    let type: String
    let modelsDir: String?
}

enum Output {
    private static let lock = NSLock()

    static func send(_ event: [String: Any]) {
        guard var data = try? JSONSerialization.data(withJSONObject: event) else { return }
        data.append(0x0A)
        lock.lock()
        defer { lock.unlock() }
        FileHandle.standardOutput.write(data)
    }

    static func progress(stage: String, fraction: Double) {
        send(["type": "progress", "stage": stage, "fraction": max(0, min(1, fraction))])
    }

    static func failure(_ reason: String, _ message: String) {
        send(["type": "error", "reason": reason, "message": message])
    }
}
