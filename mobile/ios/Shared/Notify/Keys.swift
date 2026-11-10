import Foundation
import Security

/// The notification key this phone gave each host, in a Keychain group the app shares with its
/// notification extension. Each can only read notifications, so it opens whenever a push arrives,
/// locked or not, once the phone has been unlocked since it started.
final class Keys {
  private static let service = "sikemux.notify"
  private static let phoneAccount = "phone"
  private static let hostPrefix = "host:"
  private static let unreadablePrefix = "unreadable:"

  private let group: String?

  init(bundle: Bundle = .main) {
    group = bundle.object(forInfoDictionaryKey: "SikemuxKeychainGroup") as? String
  }

  var phone: String? {
    get { read(Self.phoneAccount).flatMap { String(data: $0, encoding: .utf8) } }
    set {
      if let newValue { write(Self.phoneAccount, Data(newValue.utf8)) } else { delete(Self.phoneAccount) }
    }
  }

  func put(host: String, keyId: UInt32, key: Data) {
    write(Self.hostPrefix + host, Self.entry(keyId: keyId, key: key))
    delete(Self.unreadablePrefix + host)
  }

  /// The host's key, unless a push showed the host no longer seals with it.
  func get(host: String) -> HostKey? {
    if read(Self.unreadablePrefix + host) != nil { return nil }
    return read(Self.hostPrefix + host).flatMap { Self.hostKey(host: host, stored: $0) }
  }

  func all() -> [HostKey] {
    var query = base()
    query[kSecMatchLimit as String] = kSecMatchLimitAll
    query[kSecReturnAttributes as String] = true
    query[kSecReturnData as String] = true
    var found: CFTypeRef?
    guard SecItemCopyMatching(query as CFDictionary, &found) == errSecSuccess, let items = found as? [[String: Any]] else {
      return []
    }
    return items.compactMap { item in
      guard let account = item[kSecAttrAccount as String] as? String, account.hasPrefix(Self.hostPrefix),
            let data = item[kSecValueData as String] as? Data
      else { return nil }
      return Self.hostKey(host: String(account.dropFirst(Self.hostPrefix.count)), stored: data)
    }
  }

  func unreadable(host: String) {
    write(Self.unreadablePrefix + host, Data([1]))
  }

  func remove(host: String) {
    delete(Self.hostPrefix + host)
    delete(Self.unreadablePrefix + host)
  }

  func clear() {
    SecItemDelete(base() as CFDictionary)
  }

  /// The key id, big-endian, then the 32-byte key.
  static func entry(keyId: UInt32, key: Data) -> Data {
    withUnsafeBytes(of: keyId.bigEndian) { Data($0) } + key
  }

  static func hostKey(host: String, stored: Data) -> HostKey? {
    let bytes = [UInt8](stored)
    guard bytes.count == 4 + 32 else { return nil }
    let keyId = bytes[0..<4].reduce(UInt32(0)) { $0 << 8 | UInt32($1) }
    return HostKey(host: host, keyId: keyId, key: Data(bytes[4...]))
  }

  private func base() -> [String: Any] {
    var query: [String: Any] = [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: Self.service,
    ]
    if let group { query[kSecAttrAccessGroup as String] = group }
    return query
  }

  private func read(_ account: String) -> Data? {
    var query = base()
    query[kSecAttrAccount as String] = account
    query[kSecReturnData as String] = true
    var found: CFTypeRef?
    guard SecItemCopyMatching(query as CFDictionary, &found) == errSecSuccess else { return nil }
    return found as? Data
  }

  private func write(_ account: String, _ data: Data) {
    var query = base()
    query[kSecAttrAccount as String] = account
    let update: [String: Any] = [
      kSecValueData as String: data,
      kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
    ]
    if SecItemUpdate(query as CFDictionary, update as CFDictionary) == errSecItemNotFound {
      query.merge(update) { $1 }
      SecItemAdd(query as CFDictionary, nil)
    }
  }

  private func delete(_ account: String) {
    var query = base()
    query[kSecAttrAccount as String] = account
    SecItemDelete(query as CFDictionary)
  }
}
