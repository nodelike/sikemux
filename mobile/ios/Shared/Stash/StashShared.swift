import Foundation
import Security

/// What the app leaves for its share extension: the chats used last and where the accounts server
/// is, in the App Group both can read, and this phone's key, in a Keychain group both can read.
enum StashShared {
    private static let service = "sikemux.stash"
    private static let keyAccount = "device-key"
    private static let fileName = "share.json"

    static func appGroup(_ bundle: Bundle = .main) -> String? {
        bundle.object(forInfoDictionaryKey: "SikemuxAppGroup") as? String
    }

    static func keychainGroup(_ bundle: Bundle = .main) -> String? {
        bundle.object(forInfoDictionaryKey: "SikemuxStashKeychainGroup") as? String
    }

    private static func file() -> URL? {
        guard let group = appGroup() else { return nil }
        return FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: group)?.appendingPathComponent(
            fileName)
    }

    static func write(_ data: Data?) {
        guard let file = file() else {
            return print("sikemux: this build has no App Group to share with its extension")
        }
        do {
            if let data {
                try data.write(to: file, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
            } else if FileManager.default.fileExists(atPath: file.path) {
                try FileManager.default.removeItem(at: file)
            }
        } catch {
            print("sikemux: could not write what the share extension reads: \(error)")
        }
    }

    /// Nil when the app has written nothing, as before it is signed in.
    static func read() -> Data? {
        guard let file = file(), FileManager.default.fileExists(atPath: file.path) else { return nil }
        do {
            return try Data(contentsOf: file)
        } catch {
            print("sikemux: could not read what the app left for the share extension: \(error)")
            return nil
        }
    }

    static func setKey(_ hex: String?) {
        SecItemDelete(base() as CFDictionary)
        guard let hex else { return }
        var item = base()
        item[kSecValueData as String] = Data(hex.utf8)
        item[kSecAttrAccessible as String] = kSecAttrAccessibleWhenUnlockedThisDeviceOnly
        let added = SecItemAdd(item as CFDictionary, nil)
        if added != errSecSuccess { print("sikemux: could not give the share extension this phone's key: \(added)") }
    }

    static func key() -> String? {
        var query = base()
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne
        var found: CFTypeRef?
        guard SecItemCopyMatching(query as CFDictionary, &found) == errSecSuccess, let data = found as? Data else {
            return nil
        }
        return String(data: data, encoding: .utf8)
    }

    private static func base() -> [String: Any] {
        var query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: keyAccount,
        ]
        if let group = keychainGroup() { query[kSecAttrAccessGroup as String] = group }
        return query
    }
}
