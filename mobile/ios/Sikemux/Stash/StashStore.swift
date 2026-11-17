import Foundation
import Observation

/// What the phone knows of the account's stash, read on signing in so the composer's + can count it.
@MainActor @Observable
final class StashStore {
    enum State {
        case loading, ready, offline
    }

    static let shared = StashStore()

    /// Live events often come in a burst, such as a file and then its link preview; one read follows them.
    private static let settle: Duration = .milliseconds(300)

    private(set) var items: [StashItem] = []
    /// When the person last looked at the stash on any device; nil before they ever have.
    private(set) var seenAt: String?
    private(set) var state = State.loading

    @ObservationIgnored private var reading = 0
    @ObservationIgnored private var settling: Task<Void, Never>?

    func start() {
        Task { await refresh() }
    }

    func forget() {
        items = []
        seenAt = nil
        state = .loading
    }

    /// Reads the untaken stash again; what was shown stays while it does.
    func refresh() async {
        reading += 1
        let read = reading
        if state == .offline, items.isEmpty { state = .loading }
        do {
            var found: [StashItem] = []
            var seen: String?
            var cursor: String?
            repeat {
                let page = try await StashAPI.list(cursor: cursor)
                found += page.items
                seen = page.seenAt
                cursor = page.cursor
            } while cursor != nil
            guard read == reading else { return }
            items = found
            seenAt = seen
            state = .ready
        } catch {
            guard read == reading else { return }
            state = .offline
            print("sikemux: could not read the stash: \(error)")
        }
    }

    /// The account's stash changed somewhere: read it again once the burst of changes settles.
    func changed() {
        settling?.cancel()
        settling = Task {
            try? await Task.sleep(for: Self.settle)
            if !Task.isCancelled { await refresh() }
        }
    }

    /// Takes items out of the stash here at once, and on the server as used; one the server refused comes back on
    /// the next read.
    func take(_ ids: [String]) async {
        guard !ids.isEmpty else { return }
        items.removeAll { ids.contains($0.id) }
        await withTaskGroup(of: Void.self) { group in
            for id in ids {
                group.addTask {
                    do { try await StashAPI.take(id) } catch {
                        print("sikemux: the stash kept an item it was asked to give up: \(error)")
                    }
                }
            }
        }
    }

    func remove(_ id: String) async throws {
        let before = items
        items.removeAll { $0.id == id }
        do { try await StashAPI.delete(id) } catch {
            items = before
            throw error
        }
    }

    /// The items stashed since the person last looked at the stash, newest first.
    var new: [StashItem] {
        guard let seenAt, let seen = Format.date(iso: seenAt) else { return seenAt == nil ? items : [] }
        return items.filter { (Format.date(iso: $0.createdAt) ?? .distantPast) > seen }
    }

    /// The person has looked at the stash: what it holds now is no longer new, on any device.
    func see() async {
        guard let newest = items.first?.createdAt, !new.isEmpty else { return }
        seenAt = newest
        do { seenAt = try await StashAPI.seen(at: newest) } catch {
            print("sikemux: could not mark the stash seen on the account: \(error)")
        }
    }
}
