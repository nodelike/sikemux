import Foundation

/// The hosts an account had when this phone last looked, so only one that signs in afterwards connects by itself.
actor Arrivals {
    private struct Seen: Codable {
        var account: String
        var keys: [String]
    }

    static let shared = Arrivals()
    private let file = URL.documentsDirectory.appending(path: "seen-hosts.json")

    /// The hosts that joined the account since the phone last looked, remembering them as seen. The first look at an
    /// account, or at a list that cannot be read, only remembers what is there: those hosts wait in Devices for a tap.
    /// A host is new once, so one that turned the phone down never asks again.
    func hostsArrived(account: String, hosts: [AccountDevice]) -> [AccountDevice] {
        guard let seen = read(), seen.account == account else {
            save(Seen(account: account, keys: hosts.map(\.key)))
            return []
        }
        let known = Set(seen.keys)
        let arrived = hosts.filter { !known.contains($0.key) }
        if !arrived.isEmpty { save(Seen(account: account, keys: seen.keys + arrived.map(\.key))) }
        return arrived
    }

    /// The hosts seen last time; none on a first look or when the list cannot be read.
    private func read() -> Seen? {
        guard FileManager.default.fileExists(atPath: file.path) else { return nil }
        do {
            return try JSONDecoder().decode(Seen.self, from: Data(contentsOf: file))
        } catch {
            print("sikemux: could not read the hosts seen on the account: \(error)")
            return nil
        }
    }

    private func save(_ seen: Seen) {
        do {
            try JSONEncoder().encode(seen).write(to: file, options: .atomic)
        } catch {
            print("sikemux: could not keep the hosts seen on the account: \(error)")
        }
    }
}
