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
        let seen = (try? Data(contentsOf: file)).flatMap { try? JSONDecoder().decode(Seen.self, from: $0) }
        guard let seen, seen.account == account else {
            save(Seen(account: account, keys: hosts.map(\.key)))
            return []
        }
        let known = Set(seen.keys)
        let arrived = hosts.filter { !known.contains($0.key) }
        if !arrived.isEmpty { save(Seen(account: account, keys: seen.keys + arrived.map(\.key))) }
        return arrived
    }

    private func save(_ seen: Seen) {
        do {
            try JSONEncoder().encode(seen).write(to: file, options: .atomic)
        } catch {
            print("sikemux: could not keep the hosts seen on the account: \(error)")
        }
    }
}
