import Foundation
import Observation
import SikemuxCore

/// The relays hosts listen on, from the accounts server's `GET /v1/network`, and whether this app is too old for
/// it. Relays rarely change, so a saved copy is used at once while the server is asked behind it.
@MainActor @Observable
final class Network {
    struct UpdateRequired: Equatable {
        let current: String
        let minimum: String
    }

    static let shared = Network()

    /// Where hosts listen when nothing better is known. Never iroh's public relays: no host listens there.
    static let defaultRelays = [Relay(url: "https://relay.sikemux.com/", region: "default", quicPort: 7842)]
    private static let freshFor: TimeInterval = 5 * 60
    private static let unreachedFor: TimeInterval = 30

    private(set) var updateRequired: UpdateRequired?
    private var askedAt: Date?
    private var lastFresh = false
    private var reading: Task<[Relay], Never>?

    private let saved = URL.documentsDirectory.appending(path: "network.json")

    /// The relays to dial through, best first.
    func relays() async -> [Relay] {
        if let copy = savedRelays(), !copy.isEmpty {
            if needsAsking { _ = ask() }
            return copy
        }
        return await (needsAsking || reading == nil ? ask() : reading!).value
    }

    func relaySettings() async -> [RelaySetting] {
        await relays().map { RelaySetting(url: $0.url, quicPort: $0.quicPort.map(UInt16.init)) }
    }

    /// Asks the server again on the next connection, unless its last answer came through.
    func askAgain() {
        if !lastFresh { askedAt = nil }
    }

    private var needsAsking: Bool {
        guard let askedAt else { return true }
        return Date().timeIntervalSince(askedAt) > (lastFresh ? Self.freshFor : Self.unreachedFor)
    }

    private func ask() -> Task<[Relay], Never> {
        askedAt = Date()
        let task = Task { await read() }
        reading = task
        return task
    }

    private func read() async -> [Relay] {
        var request = URLRequest(url: Config.apiUrl.appending(path: "v1/network"))
        request.timeoutInterval = 3
        if let (data, response) = try? await URLSession.shared.data(for: request),
            (response as? HTTPURLResponse)?.statusCode == 200,
            let answer = try? JSONDecoder().decode(WireNetwork.self, from: data)
        {
            check(answer.minimumVersions.ios)
            let relays = answer.relays.filter { $0.url.hasPrefix("http://") || $0.url.hasPrefix("https://") }
            if !relays.isEmpty {
                try? data.write(to: saved, options: .atomic)
                lastFresh = true
                return relays
            }
        }
        lastFresh = false
        return savedRelays().flatMap { $0.isEmpty ? nil : $0 } ?? Self.defaultRelays
    }

    private func savedRelays() -> [Relay]? {
        guard let data = try? Data(contentsOf: saved) else { return nil }
        return (try? JSONDecoder().decode(WireNetwork.self, from: data))?.relays
    }

    /// Only an answer from the server decides this app is too old; failing to reach it never does. Development
    /// builds are never too old.
    private func check(_ minimum: ChannelVersions) {
        guard !Config.isDev, let installed = Versions.fromBuild(Config.build) else { return }
        let oldest = installed.contains("-") ? minimum.nightly : minimum.stable
        guard oldest != "0.0.0", let order = Versions.compare(installed, oldest), order < 0 else {
            updateRequired = nil
            return
        }
        updateRequired = UpdateRequired(current: installed, minimum: oldest)
    }
}

enum Versions {
    /// The installed app's version from its build number, which release builds set to the store's version code:
    /// 0.5.0-nightly.3 is 50003, and 0.5.0 itself is 50099.
    static func fromBuild(_ build: String) -> String? {
        guard let code = Int(build), code >= 100 else { return nil }
        let base = "\(code / 1_000_000).\(code / 10_000 % 100).\(code / 100 % 100)"
        let nightly = code % 100
        return nightly == 99 ? base : "\(base)-nightly.\(nightly)"
    }

    /// Semantic version order: negative when `a` is older.
    static func compare(_ a: String, _ b: String) -> Int? {
        guard let left = parse(a), let right = parse(b) else { return nil }
        for index in 0..<3 where left.core[index] != right.core[index] { return left.core[index] - right.core[index] }
        if left.pre.isEmpty || right.pre.isEmpty { return right.pre.count - left.pre.count }
        for (x, y) in zip(left.pre, right.pre) {
            let order: Int
            switch (Int(x), Int(y)) {
            case (.some(let m), .some(let n)): order = m - n
            case (.some, .none): order = -1
            case (.none, .some): order = 1
            default: order = x < y ? -1 : x > y ? 1 : 0
            }
            if order != 0 { return order }
        }
        return left.pre.count - right.pre.count
    }

    private static func parse(_ version: String) -> (core: [Int], pre: [String])? {
        let parts = version.split(separator: "-", maxSplits: 1)
        let core = parts[0].split(separator: ".").compactMap { Int($0) }
        guard core.count == 3 else { return nil }
        return (core, parts.count > 1 ? parts[1].split(separator: ".").map(String.init) : [])
    }
}
