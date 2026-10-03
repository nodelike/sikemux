import Foundation

struct Request: Decodable {
    let id: Int
    let type: String
    let udid: String?
    let x: Double?
    let y: Double?
    let toX: Double?
    let toY: Double?
    let duration: Double?
    let label: String?
    let text: String?
    let button: String?
    let path: String?
    let bundleId: String?
    let arguments: [String]?
    let environment: [String: String]?
    let url: String?
    let points: [TouchPoint]?
    let process: String?
    let after: Int?
    let limit: Int?
    let fps: Int?
    let scale: Double?
    let format: String?
    let wait: Double?
    let phase: String?
    let x2: Double?
    let y2: Double?
    let orientation: String?
    let pointSize: Bool?
    let mask: String?
}

/// One moment of a touch path: where the finger is, or both fingers for a pinch, `t` seconds after it starts.
struct TouchPoint: Decodable {
    let x: Double
    let y: Double
    let x2: Double?
    let y2: Double?
    let t: Double
}

enum TouchPhase: String {
    case down, move, up
}

struct Failure: Error {
    let reason: String
    let message: String
}

enum Output {
    private static let lock = NSLock()

    static func send(_ message: [String: Any]) {
        guard var data = try? JSONSerialization.data(withJSONObject: message) else { return }
        data.append(0x0A)
        lock.lock()
        defer { lock.unlock() }
        FileHandle.standardOutput.write(data)
    }

    static func result(_ id: Int, _ fields: [String: Any] = [:]) {
        send(fields.merging(["id": id, "type": "result"]) { _, new in new })
    }

    static func failure(_ id: Int?, _ reason: String, _ message: String) {
        var message: [String: Any] = ["type": "error", "reason": reason, "message": message]
        if let id { message["id"] = id }
        send(message)
    }
}
