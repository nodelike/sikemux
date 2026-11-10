import Foundation
import SikemuxCore

extension Connection {
    /// One of the core's calls, typed by the table `scripts/generate-wire-types.mjs` writes from its Rust.
    func call<Call: PhoneCallable>(_ call: Call.Type, _ params: Call.Params) async throws -> Call.Answer {
        let sent = String(decoding: try JSONEncoder().encode(params), as: UTF8.self)
        let answer = try await self.call(method: Call.method, paramsJson: sent)
        return try JSONDecoder().decode(Call.Answer.self, from: Data(answer.utf8))
    }
}

extension JSONValue {
    /// What a call's free-form answer holds, read as `T`.
    func decoded<T: Decodable>(as type: T.Type) throws -> T {
        try JSONDecoder().decode(type, from: JSONEncoder().encode(self))
    }
}
