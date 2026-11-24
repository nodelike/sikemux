import Foundation
import SikemuxCore

/// This phone's key, made once and kept in the Keychain, and the phone on the network made from it.
@MainActor
final class Identity {
    static let shared = Identity()

    private static let keyItem = "sikemux.device-key"
    /// Coming online takes a second; one that has not by now never will, and the next call starts again.
    private static let onlineWait: Duration = .seconds(12)
    /// Cap endpoint close so a wedged socket after sleep never holds up the next bind.
    private static let closeWait: Duration = .milliseconds(200)

    private var identity: DeviceIdentity?
    private var online: Task<Device, Error>?
    private var closing: Task<Void, Never>?
    private var generation: UInt64 = 0
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
        let gen = generation
        let previousClose = closing
        let ended = Ending()
        let coming = Task { () throws -> Device in
            defer { ended.set = true }
            await previousClose?.value
            if self.generation != gen {
                throw MobileError.Connection(message: String(localized: "The network changed."))
            }
            if Network.shared.updateRequired != nil {
                throw MobileError.Outdated(macIsOlder: false)
            }
            let made = try await Device.create(identity: me, relays: await Network.shared.relaySettings())
            if self.generation != gen {
                Task { await made.close() }
                throw MobileError.Connection(message: String(localized: "The network changed."))
            }
            return made
        }
        online = coming
        Task { [weak self] in
            try? await Task.sleep(for: Self.onlineWait)
            guard let self, online == coming, !ended.set else { return }
            online = nil
            Task { await (try? coming.value)?.close() }
        }
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

    /// Takes the phone off the network while the app is away; the next call to `device` waits for close (bounded)
    /// and brings it back.
    @discardableResult
    func goOffline() -> Task<Void, Never>? {
        guard joining == 0 else { return closing }
        generation &+= 1
        guard let going = online else { return closing }
        online = nil
        let prior = closing
        let next = Task {
            await prior?.value
            _ = await raceTimeout(for: Self.closeWait) {
                guard let device = try? await going.value else { return }
                await device.close()
            }
        }
        closing = next
        return next
    }

    /// Makes this phone a new device: the account may still hold the old key, and anything it signed for that
    /// account must not follow the phone into another one.
    func rotateKey() async {
        Keychain.app.delete(Self.keyItem)
        identity = nil
        await goOffline()?.value
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

@MainActor
private final class Ending {
    var set = false
}

enum IdentityError: LocalizedError {
    case damaged
    var errorDescription: String? {
        String(localized: "This phone's key is damaged; reinstall the app to connect again.")
    }
}
