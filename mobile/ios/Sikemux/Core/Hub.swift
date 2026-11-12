import Foundation
import Network
import Observation
import SikemuxCore
import UIKit

/// What changed in a chat, as the host numbered it: a list of the core's `ChatChange` as JSON.
struct ChatDelivery: Sendable {
    let agentId: String
    let seq: UInt64
    let changesJson: String
}

/// Which side needs a newer Sikemux before the two can talk.
enum Outdated: Equatable {
    case host, phone
}

/// One paired host as the phone holds it: its connection while one is open, and everything it last sent.
@MainActor @Observable
final class HostLive {
    enum Status: Equatable {
        /// `problem` is why the last try failed, while the phone tries again.
        case connecting(problem: String?)
        case open
        case closed(problem: String, blocked: Blocked? = nil)
    }

    /// Why a closed host stays closed until someone acts.
    enum Blocked: Equatable {
        /// The host no longer knows this phone; only forgetting it and joining again helps.
        case unpaired
        case outdated(Outdated)
    }

    let core: String
    fileprivate(set) var status: Status = .connecting(problem: nil)
    /// Everything the phone shows of the host, as it last sent it; kept after the connection ends.
    fileprivate(set) var view: DeviceView?
    /// The agents the host can run, which every screen names and draws agents from.
    fileprivate(set) var catalog: [CatalogAgent] = []
    fileprivate(set) var host: HostInfo?
    @ObservationIgnored fileprivate(set) var connection: Connection?

    @ObservationIgnored fileprivate var watchers = 0
    /// Failed tries in a row, which sets how long the next one waits.
    @ObservationIgnored fileprivate var attempt = 0
    @ObservationIgnored fileprivate var openedAt: Date?
    @ObservationIgnored fileprivate var failure: String?
    /// A connection that was up dropped without the phone asking, and it is being made again.
    @ObservationIgnored fileprivate var recovering = false
    @ObservationIgnored fileprivate var opening: Task<Void, Never>?
    @ObservationIgnored fileprivate var current: ListenerBox?
    @ObservationIgnored fileprivate var retrying: Task<Void, Never>?
    @ObservationIgnored fileprivate var lingering: Task<Void, Never>?
    @ObservationIgnored fileprivate var chats: [UUID: @MainActor ([ChatDelivery]) -> Void] = [:]
    @ObservationIgnored fileprivate var catalogRead = false
    @ObservationIgnored fileprivate var heldView: DeviceView?
    /// No view has come on this connection yet; its first may settle cards that arrived while the phone was away.
    @ObservationIgnored fileprivate var awaitingView = true

    init(core: String) { self.core = core }

    var isOpen: Bool { status == .open }

    /// The host no longer knows this phone, or speaks another version; it stays so until someone acts.
    var settled: Bool {
        if case .closed(_, let blocked) = status { return blocked != nil }
        return false
    }
}

/// The paired hosts and a live connection to each one a screen is watching. The core pushes its `DeviceView`
/// whenever it changes; nothing here polls.
@MainActor @Observable
final class Hub {
    static let shared = Hub()

    /// A host nobody is looking at keeps its connection this long, for a quick return.
    private static let linger: Duration = .seconds(30)
    private static let retryWaits: [Double] = [1, 3, 8, 15, 30]
    /// A connection that lasted this long dropped by chance; one that drops sooner counts as another failed try.
    private static let steady: TimeInterval = 20
    /// Tries a dropped connection gets before the host is shown as unreachable; until then it reads as reconnecting.
    private static let patientTries = 3
    /// A host that does not answer the unpair in this time is forgotten on the phone anyway.
    private static let unpairWait: Duration = .seconds(3)
    /// Glancing at another app keeps the connections; staying away longer lets them go.
    private static let awayAfter: TimeInterval = 10
    /// After the phone changes network, a connection that cannot answer in this time is replaced.
    private static let checkWait: Duration = .seconds(5)
    private static let damagedProblem =
        "The list of paired hosts couldn't be read, so it was started again. Connect your hosts again."

    private(set) var devices: [PairedDevice] = []
    private(set) var loaded = false
    private(set) var problem: String?

    private var hosts: [String: HostLive] = [:]
    /// Hosts left for good; a screen still open on one must not reconnect to it.
    private var forgotten: Set<String> = []
    private var away = false
    private var backgrounded = false
    private var leftAt = Date.distantPast
    private var leaving: Task<Void, Never>?
    private let path = NWPathMonitor()
    private var lastPath: NWPath.Status?

    private init() {
        let center = NotificationCenter.default
        center.addObserver(forName: UIApplication.didEnterBackgroundNotification, object: nil, queue: .main) { _ in
            MainActor.assumeIsolated { Hub.shared.wentAway() }
        }
        center.addObserver(forName: UIApplication.didBecomeActiveNotification, object: nil, queue: .main) { _ in
            MainActor.assumeIsolated { Hub.shared.cameBack() }
        }
        path.pathUpdateHandler = { update in
            let status = update.status
            Task { @MainActor in Hub.shared.networkChanged(status) }
        }
        path.start(queue: .global(qos: .utility))
    }

    // MARK: Paired hosts

    func reloadDevices() async {
        let store = PairedStore.shared
        devices = await store.all()
        problem = await store.damaged ? Self.damagedProblem : nil
        loaded = true
        HostJoins.shared.joinNew()
    }

    /// Reads the list of paired hosts from disk again, clearing the problem it showed if it now reads cleanly.
    func readDevicesAgain() async {
        await PairedStore.shared.readAgain()
        await reloadDevices()
    }

    func device(_ core: String) -> PairedDevice? {
        devices.first { $0.core == core }
    }

    func remember(_ device: PairedDevice) async {
        await PairedStore.shared.remember(device)
        await reloadDevices()
    }

    func update(_ core: String, _ edit: @Sendable (inout PairedDevice) -> Void) async {
        await PairedStore.shared.update(core, edit)
        await reloadDevices()
    }

    // MARK: Live hosts

    /// One host's live state, made on first use. A forgotten host's is never kept, so nothing brings it back.
    func live(_ core: String) -> HostLive {
        if let found = hosts[core] { return found }
        let made = HostLive(core: core)
        if forgotten.contains(core) {
            made.status = .closed(problem: "Forgotten.")
        } else {
            hosts[core] = made
        }
        return made
    }

    /// The hosts this phone holds a connection to now.
    var openConnections: [(core: String, connection: Connection)] {
        hosts.compactMap { core, live in live.connection.map { (core, $0) } }
    }

    /// Keeps a host connected until the returned watch is ended; screens hold one while they show.
    func watch(_ core: String) -> HostWatch {
        let found = live(core)
        found.watchers += 1
        found.lingering?.cancel()
        if found.status != .open && !found.settled { open(core) }
        return HostWatch { [weak self] in self?.unwatch(core) }
    }

    private func unwatch(_ core: String) {
        let found = live(core)
        found.watchers -= 1
        guard found.watchers == 0 else { return }
        found.lingering = Task { [weak self] in
            try? await Task.sleep(for: Self.linger)
            guard !Task.isCancelled, found.watchers == 0 else { return }
            self?.close(found, reason: "Not in use.")
        }
    }

    func retry(_ core: String) {
        let found = live(core)
        found.retrying?.cancel()
        found.attempt = 0
        open(core)
    }

    /// A host this phone just joined again: forgotten no longer, and worth trying however it last turned it away.
    func rejoined(_ core: String) {
        forgotten.remove(core)
        let found = live(core)
        guard found.status != .open else { return }
        found.attempt = 0
        if found.watchers > 0 { open(core) } else { found.status = .connecting(problem: nil) }
    }

    /// Leaves a host for good: tells it to unpair this phone while it can, then drops the connection and the host.
    /// `beforeUnpair` runs first with the open connection, so the phone can withdraw what it gave the host.
    func forget(_ core: String, beforeUnpair: ((Connection) async -> Void)? = nil) async {
        forgotten.insert(core)
        let found = hosts[core]
        if let connection = found?.connection {
            let goodbye = Task {
                await Notify.shared.withdrawKey(core, connection)
                await beforeUnpair?(connection)
                try? await connection.unpair()
            }
            await withTaskGroup(of: Void.self) { group in
                group.addTask { await goodbye.value }
                group.addTask { try? await Task.sleep(for: Self.unpairWait) }
                await group.next()
                group.cancelAll()
            }
        }
        if let found {
            release(found)
            found.status = .closed(problem: "Forgotten.")
        }
        hosts[core] = nil
        Keys().remove(host: core)
        Cards.dismiss(host: core)
        await PairedStore.shared.forget(core)
        await reloadDevices()
    }

    /// A chat's events from this host, in the batches they arrived in, until the returned watch is ended.
    func onChatEvents(_ core: String, _ listen: @escaping @MainActor ([ChatDelivery]) -> Void) -> HostWatch {
        let found = live(core)
        let id = UUID()
        found.chats[id] = listen
        return HostWatch { found.chats[id] = nil }
    }

    // MARK: Connecting

    private func open(_ core: String) {
        let found = live(core)
        guard !forgotten.contains(core), !away, found.status != .open, found.opening == nil else { return }
        found.opening = Task { [weak self] in
            await self?.connect(found)
            found.opening = nil
        }
    }

    private func connect(_ found: HostLive) async {
        found.retrying?.cancel()
        found.status = .connecting(problem: found.recovering ? nil : found.failure)
        let box = ListenerBox(core: found.core)
        found.current = box
        let connection: Connection
        do {
            let device = try await Identity.shared.device()
            connection = try await device.connect(core: found.core, listener: box)
        } catch {
            if found.current === box { drop(found, reason: Sikemux.problem(error), error: error) }
            return
        }
        guard found.current === box, found.watchers > 0, !forgotten.contains(found.core), !away else {
            connection.close()
            if found.current === box {
                found.current = nil
                found.status = .closed(problem: "Not in use.")
            }
            return
        }
        found.openedAt = Date()
        found.failure = nil
        found.recovering = false
        found.connection = connection
        found.status = .open
        found.awaitingView = true
        Task { await Notify.shared.shareKey(found.core, connection) }
        if UIApplication.shared.applicationState != .active {
            Task { try? await connection.setForeground(foreground: false) }
        }
        readCatalog(found, connection)
        Task { await readHost(found, connection) }
    }

    /// Takes the agents the host can run, then shows the view that waited on them. A failed read is tried again,
    /// waiting longer each time, for as long as this connection is the host's.
    private func readCatalog(_ found: HostLive, _ connection: Connection) {
        Task {
            var wait: Duration = .seconds(1)
            while true {
                do {
                    found.catalog = try await connection.call(PhoneCall.AgentCatalog.self, NoParams())
                    break
                } catch {
                    print("sikemux: could not read the host's agents: \(error)")
                }
                showHeldView(found)
                try? await Task.sleep(for: wait)
                guard found.connection === connection else { return }
                wait = min(wait * 2, .seconds(30))
            }
            showHeldView(found)
        }
    }

    private func showHeldView(_ found: HostLive) {
        found.catalogRead = true
        if let held = found.heldView {
            found.heldView = nil
            found.view = held
        }
    }

    private func readHost(_ found: HostLive, _ connection: Connection) async {
        guard let json = try? await connection.host(),
            let host = try? JSONDecoder().decode(HostInfo.self, from: Data(json.utf8))
        else { return }
        found.host = host
        let known = device(found.core)
        let access = host.access ?? known?.access
        guard known?.model != host.model || known?.channel != host.channel || known?.access != access else { return }
        await update(found.core) {
            if let access { $0.access = access }
            $0.model = host.model
            $0.channel = host.channel
        }
    }

    fileprivate func deliver(_ box: ListenerBox, chats: [ChatDelivery], view: DeviceView?) {
        guard let found = hosts[box.core], found.current === box else { return }
        if !chats.isEmpty {
            for listen in found.chats.values { listen(chats) }
        }
        guard let view else { return }
        Cards.reconcile(host: found.core, view: view, fresh: found.awaitingView)
        found.awaitingView = false
        ShareExtension.changed()
        Glances.changed()
        if found.catalogRead {
            if found.view != view { found.view = view }
        } else {
            found.heldView = view
        }
    }

    fileprivate func closed(_ box: ListenerBox) {
        guard let found = hosts[box.core], found.current === box, found.status == .open else { return }
        found.recovering = true
        drop(found, reason: "The connection closed.")
    }

    private func release(_ found: HostLive) {
        found.retrying?.cancel()
        found.lingering?.cancel()
        found.current = nil
        if let connection = found.connection {
            connection.close()
            found.connection = nil
            if !forgotten.contains(found.core) {
                let now = Date().timeIntervalSince1970 * 1000
                Task { await update(found.core) { $0.lastSeen = now } }
            }
        }
        if let openedAt = found.openedAt, Date().timeIntervalSince(openedAt) >= Self.steady { found.attempt = 0 }
        found.openedAt = nil
    }

    private func drop(_ found: HostLive, reason: String, error: Error? = nil) {
        release(found)
        let blocked: HostLive.Blocked? =
            switch error as? MobileError {
            case .Unpaired: .unpaired
            case .Outdated(let macIsOlder): .outdated(macIsOlder ? .host : .phone)
            default: nil
            }
        found.failure = reason
        let holding =
            found.recovering && blocked == nil && found.attempt < Self.patientTries && found.watchers > 0 && !away
        if !holding { found.recovering = false }
        found.status = holding ? .connecting(problem: nil) : .closed(problem: reason, blocked: blocked)
        if !found.settled { scheduleRetry(found) }
    }

    private func scheduleRetry(_ found: HostLive) {
        guard found.watchers > 0, !away else { return }
        let wait = Self.retryWaits[min(found.attempt, Self.retryWaits.count - 1)] * Double.random(in: 0.8...1.2)
        found.attempt += 1
        found.retrying = Task { [weak self] in
            try? await Task.sleep(for: .seconds(wait))
            guard !Task.isCancelled else { return }
            self?.open(found.core)
        }
    }

    private func close(_ found: HostLive, reason: String) {
        release(found)
        found.failure = nil
        found.recovering = false
        found.status = .closed(problem: reason)
    }

    /// Drops a connection that may be dead and makes a new one now, as on a fresh start.
    private func reconnect(_ found: HostLive, reason: String) {
        close(found, reason: reason)
        found.attempt = 0
        found.recovering = true
        open(found.core)
    }

    // MARK: The app coming and going

    private func tellForeground(_ foreground: Bool) {
        for (_, connection) in openConnections {
            Task { try? await connection.setForeground(foreground: foreground) }
        }
    }

    private func wentAway() {
        tellForeground(false)
        backgrounded = true
        leftAt = Date()
        leaving?.cancel()
        leaving = Task { [weak self] in
            try? await Task.sleep(for: .seconds(Self.awayAfter))
            guard let self, !Task.isCancelled else { return }
            away = true
            for found in hosts.values {
                let wasOpen = found.status == .open
                close(found, reason: "Paused while the app is away.")
                found.recovering = wasOpen
            }
            await Identity.shared.goOffline()
        }
    }

    private func cameBack() {
        guard backgrounded else { return }
        backgrounded = false
        leaving?.cancel()
        tellForeground(true)
        let wasAway = away || Date().timeIntervalSince(leftAt) > Self.awayAfter
        away = false
        for found in hosts.values {
            if let connection = found.connection, wasAway || !connection.isOpen() {
                close(found, reason: "Reconnecting.")
                found.recovering = true
            }
            guard found.watchers > 0, found.status != .open else { continue }
            if case .closed(_, .unpaired) = found.status { continue }
            found.attempt = 0
            open(found.core)
        }
    }

    private func networkChanged(_ status: NWPath.Status) {
        defer { lastPath = status }
        guard lastPath != nil, status != lastPath || status == .satisfied, !away else { return }
        if status == .satisfied { Network.shared.askAgain() }
        for found in hosts.values {
            if let connection = found.connection {
                check(found, connection)
            } else if found.watchers > 0, !found.settled {
                found.attempt = 0
                open(found.core)
            }
        }
    }

    /// Asks a connection to answer after the network changed under it, and replaces it when it cannot.
    private func check(_ found: HostLive, _ connection: Connection) {
        guard connection.isOpen() else {
            reconnect(found, reason: "The network changed.")
            return
        }
        Task {
            let answered = await withTaskGroup(of: Bool.self) { group in
                group.addTask { (try? await connection.host()) != nil }
                group.addTask {
                    try? await Task.sleep(for: Self.checkWait)
                    return false
                }
                let first = await group.next() ?? false
                group.cancelAll()
                return first
            }
            if !answered, found.connection === connection { reconnect(found, reason: "The network changed.") }
        }
    }
}

/// Ends a watch when it is ended by hand or let go.
final class HostWatch {
    private var end: (@MainActor () -> Void)?

    init(_ end: @escaping @MainActor () -> Void) { self.end = end }

    @MainActor func cancel() {
        end?()
        end = nil
    }

    deinit {
        guard let end else { return }
        Task { @MainActor in end() }
    }
}

/// Hears one connection's events on the bridge's own thread, reads the views there, and hands them to the hub.
final class ListenerBox: CoreListener {
    let core: String

    init(core: String) { self.core = core }

    func events(events: [CoreEvent]) throws {
        var chats: [ChatDelivery] = []
        var viewJson: String?
        for event in events {
            switch event {
            case .chat(let agentId, let seq, let changesJson):
                chats.append(ChatDelivery(agentId: agentId, seq: seq, changesJson: changesJson))
            case .view(let json):
                viewJson = json
            }
        }
        var view: DeviceView?
        if let viewJson {
            do {
                view = try JSONDecoder().decode(DeviceView.self, from: Data(viewJson.utf8))
            } catch {
                print("sikemux: could not read the host's view: \(error)")
            }
        }
        guard !chats.isEmpty || view != nil else { return }
        DispatchQueue.main.async {
            MainActor.assumeIsolated { Hub.shared.deliver(self, chats: chats, view: view) }
        }
    }

    func closed() throws {
        DispatchQueue.main.async {
            MainActor.assumeIsolated { Hub.shared.closed(self) }
        }
    }
}
