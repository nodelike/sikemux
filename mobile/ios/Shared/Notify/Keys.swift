import Foundation

/// The notification key this phone gave each host, in a Keychain group the app shares with its
/// notification extension. Each can only read notifications, so it opens whenever a push arrives,
/// locked or not, once the phone has been unlocked since it started.
final class Keys {
    private static let phoneAccount = "phone"
    private static let hostPrefix = "host:"
    private static let unreadablePrefix = "unreadable:"

    private let keychain: Keychain

    init(bundle: Bundle = .main) {
        keychain = Keychain(
            service: "sikemux.notify", group: bundle.object(forInfoDictionaryKey: "SikemuxKeychainGroup") as? String,
            opens: .afterFirstUnlock)
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
        do {
            return try keychain.all().compactMap { item in
                guard item.account.hasPrefix(Self.hostPrefix) else { return nil }
                return Self.hostKey(host: String(item.account.dropFirst(Self.hostPrefix.count)), stored: item.data)
            }
        } catch {
            print("sikemux: could not read the notification keys: \(error)")
            return []
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
        keychain.clear()
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

    private func read(_ account: String) -> Data? {
        do {
            return try keychain.data(account)
        } catch {
            print("sikemux: could not read a notification key: \(error)")
            return nil
        }
    }

    private func write(_ account: String, _ data: Data) {
        do { try keychain.write(account, data) } catch { print("sikemux: could not keep a notification key: \(error)") }
    }

    private func delete(_ account: String) {
        keychain.delete(account)
    }

}
