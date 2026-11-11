import CryptoKit
import Foundation

/// A shared chat's link: which share, and the key that reads it. The key rides in the link's fragment, so it never
/// reaches a server.
struct SharedChatLink: Hashable {
    let id: String
    let key: String

    init?(id: String, key: String?) {
        guard let key, id.wholeMatch(of: /[A-Za-z0-9_-]{22}/) != nil, key.wholeMatch(of: /[A-Za-z0-9_-]{43}/) != nil,
            let bytes = Data(base64URL: key)
        else { return nil }
        self.id = id
        self.key = key
        secret = SymmetricKey(data: bytes)
    }

    private let secret: SymmetricKey

    static func == (a: SharedChatLink, b: SharedChatLink) -> Bool { a.id == b.id && a.key == b.key }
    func hash(into hasher: inout Hasher) { hasher.combine(id) }

    private var aad: Data { Data("sikemux-share|v1|\(id)".utf8) }

    /// The text the host sealed: a 12-byte nonce, then AES-256-GCM ciphertext and tag. Nil when this link's key did
    /// not seal it.
    func open(_ sealed: String) -> String? {
        guard let bytes = Data(base64URL: sealed), let box = try? AES.GCM.SealedBox(combined: bytes),
            let plain = try? AES.GCM.open(box, using: secret, authenticating: aad)
        else { return nil }
        return String(data: plain, encoding: .utf8)
    }

    func seal(_ text: String) -> String? {
        (try? AES.GCM.seal(Data(text.utf8), using: secret, authenticating: aad))?.combined?.base64URL
    }

    /// What shows the server this viewer holds the link, without giving it the key.
    var viewerProof: String {
        Data(HMAC<SHA256>.authenticationCode(for: Data("sikemux-share-viewer|v1|\(id)".utf8), using: secret)).base64URL
    }
}
