import Foundation

/// What the accounts server says to a share's viewer, as `server/protocol/schema/share.json` defines it.
enum ShareServerMessage: Decodable {
    struct Ready: Decodable {
        let hostOnline: Bool?
        let expiresAt: String?
        let authExpiresAt: String?
    }

    struct Frame: Decodable {
        let seq: Int
        let sealed: String
    }

    enum EndReason: String, Decodable {
        case stopped, expired
    }

    case challenge
    case ready(Ready)
    case frames([Frame])
    case answer(sealed: String)
    case host(online: Bool)
    case ended(reason: EndReason?)
    case ping
    /// One added after this build, or meant for a sharing host.
    case other

    private enum Keys: String, CodingKey {
        case type, frames, sealed, online, reason
    }

    init(from decoder: Decoder) throws {
        let fields = try decoder.container(keyedBy: Keys.self)
        switch try fields.decode(String.self, forKey: .type) {
        case "challenge": self = .challenge
        case "ready": self = .ready(try Ready(from: decoder))
        case "frames": self = .frames(try fields.decode([Frame].self, forKey: .frames))
        case "answer": self = .answer(sealed: try fields.decode(String.self, forKey: .sealed))
        case "host": self = .host(online: try fields.decode(Bool.self, forKey: .online))
        case "ended": self = .ended(reason: try? fields.decode(EndReason.self, forKey: .reason))
        case "ping": self = .ping
        default: self = .other
        }
    }
}

/// What a share's viewer says to the accounts server.
enum ShareClientMessage: Encodable {
    case hello(token: String, share: String, proof: String, since: Int?)
    case auth(token: String)
    case ask(sealed: String)
    case pong

    private enum Keys: String, CodingKey {
        case type, role, token, share, proof, since, sealed
    }

    func encode(to encoder: Encoder) throws {
        var fields = encoder.container(keyedBy: Keys.self)
        switch self {
        case .hello(let token, let share, let proof, let since):
            try fields.encode("hello", forKey: .type)
            try fields.encode("viewer", forKey: .role)
            try fields.encode(token, forKey: .token)
            try fields.encode(share, forKey: .share)
            try fields.encode(proof, forKey: .proof)
            try fields.encodeIfPresent(since, forKey: .since)
        case .auth(let token):
            try fields.encode("auth", forKey: .type)
            try fields.encode(token, forKey: .token)
        case .ask(let sealed):
            try fields.encode("ask", forKey: .type)
            try fields.encode(sealed, forKey: .sealed)
        case .pong:
            try fields.encode("pong", forKey: .type)
        }
    }
}

/// Why the server closed a share connection, where the viewer acts on it.
enum ShareCloseCode: Int {
    /// No share has this id; never reconnect.
    case notFound = 4404
    /// The share ended after an ended message; never reconnect.
    case ended = 4410
    /// Over a limit; wait at least 30 seconds.
    case overLimit = 4429
}
