import Foundation
import Security

/// Small secrets in the Keychain, on this phone only, under one service. Secrets an extension reads too name the
/// access group the app shares with it.
struct Keychain: Sendable {
    enum Opens: Sendable {
        case whenUnlocked
        /// Once the phone has been unlocked since it started, as a push that arrives while it is locked needs.
        case afterFirstUnlock
    }

    let service: String
    var group: String?
    var opens = Opens.whenUnlocked
    /// The app's own secrets were first saved with their account as bytes, and are only found that way.
    var accountAsData = false

    /// The app's own secrets, under the service the app has always used, so an installed phone keeps its key.
    static let app = Keychain(service: "app:no-auth", accountAsData: true)

    func read(_ account: String) throws -> String? {
        try data(account).flatMap { String(data: $0, encoding: .utf8) }
    }

    func data(_ account: String) throws -> Data? {
        var item = query(account)
        item[kSecReturnData as String] = true
        item[kSecMatchLimit as String] = kSecMatchLimitOne
        var found: CFTypeRef?
        let status = SecItemCopyMatching(item as CFDictionary, &found)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess, let data = found as? Data else { throw KeychainError(status: status) }
        return data
    }

    func write(_ account: String, _ value: String) throws {
        try write(account, Data(value.utf8))
    }

    func write(_ account: String, _ data: Data) throws {
        let updated = SecItemUpdate(query(account) as CFDictionary, [kSecValueData as String: data] as CFDictionary)
        if updated == errSecSuccess { return }
        guard updated == errSecItemNotFound else { throw KeychainError(status: updated) }
        var item = query(account)
        item[kSecValueData as String] = data
        item[kSecAttrAccessible as String] =
            switch opens {
            case .whenUnlocked: kSecAttrAccessibleWhenUnlockedThisDeviceOnly
            case .afterFirstUnlock: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
            }
        let added = SecItemAdd(item as CFDictionary, nil)
        guard added == errSecSuccess else { throw KeychainError(status: added) }
    }

    func delete(_ account: String) {
        SecItemDelete(query(account) as CFDictionary)
    }

    /// Every secret under the service, by account.
    func all() throws -> [(account: String, data: Data)] {
        var item = base()
        item[kSecMatchLimit as String] = kSecMatchLimitAll
        item[kSecReturnAttributes as String] = true
        item[kSecReturnData as String] = true
        var found: CFTypeRef?
        let status = SecItemCopyMatching(item as CFDictionary, &found)
        if status == errSecItemNotFound { return [] }
        guard status == errSecSuccess, let items = found as? [[String: Any]] else {
            throw KeychainError(status: status)
        }
        return items.compactMap { item in
            guard let account = item[kSecAttrAccount as String] as? String,
                let data = item[kSecValueData as String] as? Data
            else { return nil }
            return (account, data)
        }
    }

    /// Deletes every secret under the service.
    func clear() {
        SecItemDelete(base() as CFDictionary)
    }

    private func base() -> [String: Any] {
        var item: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
        ]
        if let group { item[kSecAttrAccessGroup as String] = group }
        return item
    }

    private func query(_ account: String) -> [String: Any] {
        var item = base()
        if accountAsData {
            item[kSecAttrGeneric as String] = Data(account.utf8)
            item[kSecAttrAccount as String] = Data(account.utf8)
        } else {
            item[kSecAttrAccount as String] = account
        }
        return item
    }
}

struct KeychainError: LocalizedError {
    let status: OSStatus
    var errorDescription: String? {
        "The Keychain refused (\(SecCopyErrorMessageString(status, nil) as String? ?? "\(status)"))."
    }
}
