import Foundation
import SikemuxCore

/// This phone's key, made once and kept in the Keychain, and the phone on the network made from it.
@MainActor
final class Identity {
    static let shared = Identity()

    private static let keyItem = "sikemux.device-key"

    private var identity: DeviceIdentity?
    private var online: Task<Device, Error>?
    private var joining = 0

    /// This phone's key, for proving who it is to the accounts server; it never goes on the network.
    func deviceIdentity() throws -> DeviceIdentity {
        if let identity { return identity }
        let made = try DeviceIdentity(key: try key())
        identity = made
        return made
    }

    /// This phone's key in hex, for the share extension, which signs its requests to the accounts server with it.
    func keyHex() throws -> String {
        try key().hex
    }

    /// This phone on the network. A failure is not kept, so the next call tries again.
    func device() async throws -> Device {
        if let online { return try await online.value }
        let me = try deviceIdentity()
        let coming = Task { () throws -> Device in
            if Network.shared.updateRequired != nil {
                throw MobileError.Outdated(macIsOlder: false)
            }
            return try await Device.create(identity: me, relays: await Network.shared.relaySettings())
        }
        online = coming
        do {
            return try await coming.value
        } catch {
            if online == coming { online = nil }
            throw error
        }
    }

    /// Keeps the phone online while a host decides whether to let it in, which can outlast the app being in front.
    func whileJoining<T>(_ work: (Device) async throws -> T) async throws -> T {
        joining += 1
        defer { joining -= 1 }
        return try await work(try await device())
    }

    /// Takes the phone off the network while the app is away; the next call to `device` brings it back.
    func goOffline() async {
        guard let going = online, joining == 0 else { return }
        online = nil
        await (try? going.value)?.close()
    }

    /// Makes this phone a new device: the account may still hold the old key, and anything it signed for that
    /// account must not follow the phone into another one.
    func rotateKey() async {
        Keychain.app.delete(Self.keyItem)
        identity = nil
        await goOffline()
    }

    private func key() throws -> Data {
        if let stored = try Keychain.app.read(Self.keyItem) {
            guard stored.count == 64, let bytes = Data(hex: stored) else {
                throw IdentityError.damaged
            }
            return bytes
        }
        let key = newDeviceKey()
        try Keychain.app.write(Self.keyItem, key.hex)
        return key
    }
}

enum IdentityError: LocalizedError {
    case damaged
    var errorDescription: String? {
        String(localized: "This phone's key is damaged; reinstall the app to connect again.")
    }
}
