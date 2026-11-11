import Foundation

/// A computer this phone paired with, known by its core's key.
struct PairedDevice: Codable, Hashable, Identifiable {
    struct Backdrop: Codable, Hashable {
        struct Image: Codable, Hashable {
            let id: String
            let uri: String
        }
        var texture: Bool
        var image: Image?
    }

    let core: String
    var access: DeviceAccess
    /// Milliseconds since 1970, as the file has always kept times.
    var pairedAt: Double
    var name: String?
    var model: String?
    var channel: BuildChannel?
    /// The project the device screen is scoped to; absent shows them all.
    var project: String?
    /// The host's theme colours as last seen, so its screens open in them before it connects.
    var palette: [String: String]?
    /// The host's pane backdrop as last seen, its picture saved on the phone.
    var backdrop: Backdrop?
    var lastSeen: Double?

    var id: String { core }

    var displayName: String { name ?? "Host \(core.prefix(8))" }

    var channelLabel: String? { channel?.label }

    enum Kind { case laptop, desktop, mini }

    var kind: Kind {
        guard let model else { return .laptop }
        if model.localizedCaseInsensitiveContains("macbook") { return .laptop }
        if model.localizedCaseInsensitiveContains("mini") { return .mini }
        return .desktop
    }
}

/// The paired hosts, in `paired-devices.json` in the app's documents. Nothing in it is secret (a host's key is
/// public), so it lives in a file rather than the Keychain. Changes run one after another on the latest list.
actor PairedStore {
    static let shared = PairedStore()

    private let file = URL.documentsDirectory.appending(path: "paired-devices.json")
    private var cached: [PairedDevice]?
    private(set) var damaged = false

    func all() -> [PairedDevice] {
        if let cached { return cached }
        let read = load()
        cached = read
        return read
    }

    /// Reads the saved list from disk again. A damaged one was already moved aside, so it reads as a fresh list.
    func readAgain() {
        cached = nil
        damaged = false
    }

    func remember(_ device: PairedDevice) {
        change { $0.filter { $0.core != device.core } + [device] }
    }

    /// Changes a host still paired; one forgotten in the meantime stays forgotten.
    func update(_ core: String, _ edit: (inout PairedDevice) -> Void) {
        change { devices in
            devices.map { device in
                guard device.core == core else { return device }
                var changed = device
                edit(&changed)
                return changed
            }
        }
    }

    func forget(_ core: String) {
        change { $0.filter { $0.core != core } }
    }

    private func change(_ edit: ([PairedDevice]) -> [PairedDevice]) {
        let before = all()
        let devices = edit(before)
        guard devices != before || !FileManager.default.fileExists(atPath: file.path) else { return }
        do {
            try JSONEncoder().encode(devices).write(
                to: file, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
            cached = devices
        } catch {
            print("sikemux: could not save the paired hosts: \(error)")
        }
    }

    private func load() -> [PairedDevice] {
        guard let data = try? Data(contentsOf: file) else { return [] }
        if let devices = try? JSONDecoder().decode([PairedDevice].self, from: data) { return devices }
        damaged = true
        let aside = file.deletingLastPathComponent().appending(path: "paired-devices.damaged.json")
        try? FileManager.default.removeItem(at: aside)
        try? FileManager.default.moveItem(at: file, to: aside)
        return []
    }
}

extension BuildChannel {
    /// Builds other than the stable release are named, since one host pairs once per channel.
    var label: String? {
        switch self {
        case .dev: "Dev"
        case .nightly: "Nightly"
        case .stable, .unknown: nil
        }
    }
}
