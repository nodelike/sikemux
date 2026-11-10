import Foundation

// Opens the vector the core seals in server/protocol/vectors/push.json, as the notification
// extension does. test/notifyVector.mjs runs it with that file's path.

var failures = 0

func check(_ condition: Bool, _ what: String) {
  if !condition {
    failures += 1
    print("not ok: \(what)")
  }
}

func unhex(_ text: String) -> Data {
  Data(stride(from: 0, to: text.count, by: 2).map {
    let start = text.index(text.startIndex, offsetBy: $0)
    return UInt8(text[start..<text.index(start, offsetBy: 2)], radix: 16)!
  })
}

let vector = try JSONSerialization.jsonObject(with: Data(contentsOf: URL(fileURLWithPath: CommandLine.arguments[1]))) as! [String: Any]
let text = { (name: String) in vector[name] as! String }
let host = text("hostKey")
let phone = text("phoneKey")
let keyId = UInt32(text("keyId"), radix: 16)!
let key = HostKey(host: host, keyId: keyId, key: unhex(text("key")))
let blob = text("blob")

check(String(decoding: Envelope.aad(host: host, phone: phone, keyId: keyId), as: UTF8.self) == text("aad"), "aad")

let opened = Envelope.open(blob, phone: phone, keys: [key])
check(opened == .read(host: host, plaintext: text("plaintext")), "opens the vector")

if case let .read(_, plaintext) = opened {
  let notification = vector["notification"] as! [String: Any]
  let card = Card.parse(plaintext, host: host)
  check(card?.collapseId == notification["collapseId"] as? String, "collapse id")
  check(card?.url == notification["url"] as? String, "url")
  check(card?.detail == notification["detail"] as? String, "detail")
  check(card?.answerable == true, "answerable")
  check(card?.categoryIdentifier == "permission", "category")
  check(card?.userInfo["request"] == notification["requestId"] as? String, "request in userInfo")
  check(Card.parse(plaintext, host: phone) == nil, "a card from another host")
}

check(Envelope.open(blob, phone: host, keys: [key]) == .unreadable(host: host), "another phone")
check(
  Envelope.open(blob, phone: phone, keys: [HostKey(host: host, keyId: keyId, key: Data(repeating: 9, count: 32))])
    == .unreadable(host: host),
  "another key"
)
check(Envelope.open(blob, phone: phone, keys: [HostKey(host: host, keyId: keyId + 1, key: key.key)]) == .noKey, "no key")
check(Envelope.open("not base64", phone: phone, keys: [key]) == .unknown, "not a blob")

let stored = Keys.entry(keyId: keyId, key: key.key)
check(Keys.hostKey(host: host, stored: stored)?.keyId == keyId, "stored key id")
check(Keys.hostKey(host: host, stored: stored)?.key == key.key, "stored key")
check(Keys.hostKey(host: host, stored: Data([1, 2])) == nil, "a stored key too short")

if failures > 0 { exit(1) }
print("ok")
