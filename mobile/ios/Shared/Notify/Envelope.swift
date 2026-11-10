import CryptoKit
import Foundation

struct HostKey {
  let host: String
  let keyId: UInt32
  let key: Data
}

enum Opened: Equatable {
  case read(host: String, plaintext: String)
  /// Sealed by a newer host than this app understands.
  case unknown
  case noKey
  /// No key with the push's key id opened it: the host has a key this phone no longer does.
  case unreadable(host: String)
}

/// Opens what a host sealed for this phone: base64(version, key id, nonce, AES-256-GCM ciphertext and tag).
enum Envelope {
  static let version: UInt8 = 1
  static let nonceBytes = 12
  static let tagBytes = 16
  static let header = 1 + 4 + nonceBytes

  static func aad(host: String, phone: String, keyId: UInt32) -> Data {
    Data("sikemux-push|v1|\(host)|\(phone)|\(String(format: "%08x", keyId))".utf8)
  }

  static func open(_ blob: String, phone: String, keys: [HostKey]) -> Opened {
    guard let data = Data(base64Encoded: blob) else { return .unknown }
    let bytes = [UInt8](data)
    guard bytes.count >= header + tagBytes, bytes[0] == version else { return .unknown }
    let keyId = bytes[1...4].reduce(UInt32(0)) { $0 << 8 | UInt32($1) }
    let candidates = keys.filter { $0.keyId == keyId }
    guard let first = candidates.first else { return .noKey }
    guard
      let nonce = try? AES.GCM.Nonce(data: bytes[5..<header]),
      let box = try? AES.GCM.SealedBox(
        nonce: nonce,
        ciphertext: bytes[header..<(bytes.count - tagBytes)],
        tag: bytes[(bytes.count - tagBytes)...]
      )
    else { return .unknown }
    for candidate in candidates {
      let aad = aad(host: candidate.host, phone: phone, keyId: keyId)
      guard let opened = try? AES.GCM.open(box, using: SymmetricKey(data: candidate.key), authenticating: aad) else {
        continue
      }
      guard let text = String(data: opened, encoding: .utf8) else { return .unknown }
      return .read(host: candidate.host, plaintext: String(text.reversed().drop { $0 == " " }.reversed()))
    }
    return .unreadable(host: first.host)
  }
}
