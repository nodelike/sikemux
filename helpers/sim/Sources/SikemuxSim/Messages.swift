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
    let generation: Int?
    let limit: Int?
    let scale: Double?
    let format: String?
    let wait: Double?
    let phase: String?
    let key: String?
    let orientation: String?
    let pointSize: Bool?
}

/// One moment of a touch path: where the finger is, or both fingers for a pinch, `t` seconds after it starts.
extension Request {
    /// The request on a line, or why it cannot be read, with the request's id if that much could be,
    /// so the app hears back about it instead of waiting.
    static func read(_ line: String) -> Result<Request, UnreadableRequest> {
        let data = Data(line.utf8)
        do {
            return .success(try JSONDecoder().decode(Request.self, from: data))
        } catch {
            struct Id: Decodable { let id: Int }
            let id = try? JSONDecoder().decode(Id.self, from: data).id
            return .failure(UnreadableRequest(id: id, message: "Could not read the request (\(describe(error))): \(line.prefix(200))"))
        }
    }

    private static func describe(_ error: Error) -> String {
        guard let error = error as? DecodingError else { return Failure.describe(error) }
        switch error {
        case let .keyNotFound(key, _): return "missing \(key.stringValue)"
        case let .typeMismatch(_, context), let .valueNotFound(_, context), let .dataCorrupted(context):
            let field = context.codingPath.map(\.stringValue).joined(separator: ".")
            return field.isEmpty ? context.debugDescription : "\(field): \(context.debugDescription)"
        @unknown default: return "\(error)"
        }
    }
}

struct UnreadableRequest: Error {
    let id: Int?
    let message: String
}

struct TouchPoint: Decodable {
    let x: Double
    let y: Double
    let x2: Double?
    let y2: Double?
    let t: Double
}

struct Failure: Error {
    let reason: String
    let message: String

    /// What went wrong in words, rather than the name of an error's case.
    static func describe(_ error: Error) -> String {
        if let failure = error as? Failure { return failure.message }
        if let described = (error as? LocalizedError)?.errorDescription { return described }
        let bridged = error as NSError
        if bridged.userInfo[NSLocalizedDescriptionKey] != nil { return bridged.localizedDescription }
        return "\(error)"
    }
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
