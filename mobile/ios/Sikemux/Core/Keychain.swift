import Foundation
import Security

/// Small secrets kept in the Keychain, on this phone only, readable once it has been unlocked. Kept under the
/// service the app has always used, so an installed phone keeps its key.
enum Keychain {
    private static let service = "app:no-auth"

    private static func query(_ key: String) -> [String: Any] {
        let account = Data(key.utf8)
        return [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrGeneric as String: account,
            kSecAttrAccount as String: account,
        ]
    }

    static func read(_ key: String) throws -> String? {
        var item = query(key)
        item[kSecReturnData as String] = true
        item[kSecMatchLimit as String] = kSecMatchLimitOne
        var found: CFTypeRef?
        let status = SecItemCopyMatching(item as CFDictionary, &found)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess, let data = found as? Data else { throw KeychainError(status: status) }
        return String(data: data, encoding: .utf8)
    }

    static func write(_ key: String, _ value: String) throws {
        let data = Data(value.utf8)
        let updated = SecItemUpdate(query(key) as CFDictionary, [kSecValueData as String: data] as CFDictionary)
        if updated == errSecSuccess { return }
        guard updated == errSecItemNotFound else { throw KeychainError(status: updated) }
        var item = query(key)
        item[kSecValueData as String] = data
        item[kSecAttrAccessible as String] = kSecAttrAccessibleWhenUnlockedThisDeviceOnly
        let added = SecItemAdd(item as CFDictionary, nil)
        guard added == errSecSuccess else { throw KeychainError(status: added) }
    }

    static func delete(_ key: String) {
        SecItemDelete(query(key) as CFDictionary)
    }
}

struct KeychainError: LocalizedError {
    let status: OSStatus
    var errorDescription: String? {
        "The Keychain refused (\(SecCopyErrorMessageString(status, nil) as String? ?? "\(status)"))."
    }
}
