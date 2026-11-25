import AppKit
import Observation
import SwiftUI

/// What every island shows: the inbox in the core's device view, and the
/// actions that reach back to the core.
@Observable
final class NotchStore {
    private(set) var view = DeviceView.empty
    private(set) var entries: [Entry] = []
    private(set) var settings: NotchSettings
    private(set) var error: String?
    /// Devices waiting to be let in, oldest first.
    private(set) var devices: [PendingDevice] = []
    /// When each waiting device's request reached the island, for the time left to answer it.
    private(set) var deviceAsked: [String: Date] = [:]
    /// The calls waiting on the person from every project, newest first.
    private(set) var calls: [PersonCall] = []
    /// What the person has done on each waiting call's card.
    private(set) var drafts: [String: CallDraft] = [:]

    @ObservationIgnored let options: Options
    @ObservationIgnored private var link: CoreLink?
    @ObservationIgnored private var startedCore = false
    @ObservationIgnored private var lastCoreStart: Date?
    @ObservationIgnored private var lastConnectAttempt: Date?
    @ObservationIgnored private var reconnectAttempt = 0
    @ObservationIgnored private var reconnectTimer: DispatchWorkItem?
    @ObservationIgnored private var readyAt: Date?
    @ObservationIgnored private var resetBackoffTimer: DispatchWorkItem?
    /// The core said it was stopping on purpose, so the island waits for one to start rather
    /// than starting it again; a core that dies without a word is started.
    @ObservationIgnored private var coreStopped = false
    @ObservationIgnored private var waiter: CoreWaiter?

    private static let initialBackoff: TimeInterval = 0.25
    private static let maxBackoff: TimeInterval = 10
    private static let stableDuration: TimeInterval = 10
    private static let minCoreStartInterval: TimeInterval = 15
    private static let maxImmediateRetries = 6
    @ObservationIgnored private var questions: [AgentQuestion] = []
    @ObservationIgnored private var heard = false
    @ObservationIgnored private var errorTimer: DispatchWorkItem?
    /// The calls this island told the core it drew.
    @ObservationIgnored private var seen: Set<String> = []
    @ObservationIgnored private var drawnCalls: Set<String> = []
    @ObservationIgnored private var inFlightSeen: Set<String> = []
    @ObservationIgnored private var seenTimers: [String: DispatchWorkItem] = [:]
    @ObservationIgnored var onPeek: ((Peek) -> Void)?
    @ObservationIgnored var onSettings: (() -> Void)?
    @ObservationIgnored var onView: (() -> Void)?

    enum Peek {
        case ask(String)
        case call(String)
        case done(String)
        case connect(String)
    }

    init(options: Options) {
        self.options = options
        settings = NotchSettings()
        Haptics.enabled = settings.haptics
    }

    private func readSettings() {
        link?.request(CoreCall.Preferences()) { [weak self] result in
            guard let self, case .success(let answer) = result else { return }
            let fresh = answer.result.notch
            Haptics.enabled = fresh.haptics
            guard fresh != self.settings else { return }
            self.settings = fresh
            self.onSettings?()
        }
    }

    // MARK: Connection

    func connect() {
        reconnectTimer?.cancel()
        reconnectTimer = nil
        guard link == nil, waitForCore() else { return }
        lastConnectAttempt = Date()
        let link = CoreLink(socket: options.socket)
        link.onReady = { [weak self, weak link] in
            guard let self, let link, self.link === link else { return }
            self.readyAt = Date()
            self.coreStopped = false
            self.startedCore = false
            self.resetBackoffTimer?.cancel()
            let reset = DispatchWorkItem { [weak self, weak link] in
                guard let self, let link, self.link === link else { return }
                self.reconnectAttempt = 0
            }
            self.resetBackoffTimer = reset
            DispatchQueue.main.asyncAfter(deadline: .now() + Self.stableDuration, execute: reset)
            link.request(CoreCall.AgentCatalog()) { result in
                if case .success(let answer) = result {
                    AgentCatalog.names = Dictionary(answer.result.map { ($0.type, $0.label) }, uniquingKeysWith: { _, last in last })
                }
                link.request(CoreRequest.WatchView())
            }
            self.readSettings()
            link.request(CoreRequest.RemoteStatus()) { [weak self] result in
                if case .success(let answer) = result { self?.apply(answer.status.pending) }
            }
            self.error = nil
        }
        link.onEvent = { [weak self] event in self?.receive(event) }
        link.onClose = { [weak self] reason in
            guard let self else { return }
            self.resetBackoffTimer?.cancel()
            self.resetBackoffTimer = nil
            let wasStable = self.readyAt.map { Date().timeIntervalSince($0) >= Self.stableDuration } ?? false
            self.readyAt = nil
            self.link = nil
            self.disconnected()
            if let reason {
                self.fail(reason)
                _ = self.waitForCore()
                return
            }
            if wasStable { self.reconnectAttempt = 0 }
            _ = self.waitForCore()
            if self.coreStopped { return }
            self.scheduleReconnect()
        }
        if link.connect() {
            self.link = link
            waiter = nil
            return
        }
        let cooledDown = lastCoreStart.map { Date().timeIntervalSince($0) >= Self.minCoreStartInterval } ?? true
        if !startedCore, !coreStopped, cooledDown {
            startedCore = true
            lastCoreStart = Date()
            CoreStarter.start(options) { [weak self] in self?.connect() }
        }
    }

    private func scheduleReconnect() {
        reconnectTimer?.cancel()
        reconnectTimer = nil
        if reconnectAttempt >= Self.maxImmediateRetries, waiter != nil {
            return
        }
        let delay = min(Self.maxBackoff, Self.initialBackoff * pow(2.0, Double(reconnectAttempt)))
        reconnectAttempt += 1
        let work = DispatchWorkItem { [weak self] in self?.connect() }
        reconnectTimer = work
        DispatchQueue.main.asyncAfter(deadline: .now() + delay, execute: work)
    }

    private func coreWoke() {
        coreStopped = false
        guard link == nil else { return }
        if let last = lastConnectAttempt {
            let elapsed = Date().timeIntervalSince(last)
            if elapsed < Self.initialBackoff {
                reconnectTimer?.cancel()
                let work = DispatchWorkItem { [weak self] in self?.connect() }
                reconnectTimer = work
                DispatchQueue.main.asyncAfter(deadline: .now() + (Self.initialBackoff - elapsed), execute: work)
                return
            }
        }
        reconnectAttempt = 0
        connect()
    }

    /// Listens for the core starting, stopping or opening a window. It listens
    /// before each try, so a core that comes up during one still wakes it.
    private func waitForCore() -> Bool {
        if waiter != nil { return true }
        waiter = CoreWaiter(socket: options.socket) { [weak self] in self?.coreWoke() }
        if waiter == nil { fail("the notch could not wait for Sikemux's core at \(options.socket)") }
        return waiter != nil
    }

    private func receive(_ event: Event) {
        switch event {
        case .changed(topic: .stopping): coreStopped = true
        case .changed(topic: .preferences): readSettings()
        case .remote(let status): apply(status.pending)
        case .deviceView(let view): apply(view)
        case .changed, .other: break
        }
    }

    func apply(_ view: DeviceView) {
        let before = Dictionary(self.view.inbox.items.map { ($0.id, $0.kind) }, uniquingKeysWith: { first, _ in first })
        let waited = Set(calls.map(\.id))
        self.view = view
        Theme.worn.wear(view.workspace.palette)
        withAnimation(Motion.open) {
            entries = view.entries(questions)
            keepCalls(view)
        }
        onView?()
        if !Set(view.inbox.items.compactMap(\.questionId)).isSubset(of: questions.map(\.id)) { readQuestions() }
        defer { heard = true }
        guard heard else { return }
        let items = view.inbox.items
        let asks = Set(calls.map(\.id))
        if let call = calls.first(where: { !waited.contains($0.id) }) {
            onPeek?(.call(call.id))
        } else if let ask = items.last(where: {
            $0.tier == .needs && before[$0.id] == nil && !asks.contains($0.questionId ?? $0.hold?.id ?? "")
        }) {
            onPeek?(.ask(ask.id))
        } else if let done = items.last(where: { before[$0.id] == .working && $0.kind != .working }) {
            onPeek?(.done(done.id))
        }
    }

    /// The open calls, newest first, each with its card's draft; what was done on a card goes
    /// once its call does.
    private func keepCalls(_ view: DeviceView) {
        calls = (view.calls ?? []).filter { $0.state == .open }.sorted { $0.raisedAt > $1.raisedAt }
        let openIds = Set(calls.map(\.id))
        drafts = Dictionary(uniqueKeysWithValues: calls.map { ($0.id, drafts[$0.id] ?? CallDraft()) })
        seen.formIntersection(openIds)
        drawnCalls.formIntersection(openIds)
        inFlightSeen.formIntersection(openIds)
        for id in seenTimers.keys where !openIds.contains(id) {
            seenTimers.removeValue(forKey: id)?.cancel()
        }
        if link != nil {
            for call in calls where drawnCalls.contains(call.id) {
                saw(call)
            }
        }
    }

    /// A project's name, from the folder a call names it by.
    func projectName(_ path: String) -> String? {
        view.workspace.projects.first { $0.path == path }?.name
    }

    private func readQuestions() {
        link?.request(CoreCall.Questions()) { [weak self] result in
            guard case .success(let answer) = result else { return }
            self?.apply(answer.result)
        }
    }

    func apply(_ questions: [AgentQuestion]) {
        guard questions != self.questions else { return }
        self.questions = questions
        withAnimation(Motion.open) { entries = view.entries(questions) }
    }

    /// Unlike an agent's request, a device already waiting when the notch connects still opens the island.
    func apply(_ pending: [PendingDevice], now: Date = Date()) {
        let fresh = pending.filter { deviceAsked[$0.id] == nil }
        deviceAsked = Dictionary(pending.map { ($0.id, deviceAsked[$0.id] ?? now) }, uniquingKeysWith: { first, _ in first })
        if pending != devices {
            devices = pending
            onView?()
        }
        if !fresh.isEmpty, let oldest = pending.first { onPeek?(.connect(oldest.id)) }
    }

    /// Empties the island while the core is away. The first view after it comes
    /// back only catches up, so what was already asked or finished does not peek again.
    private func disconnected() {
        heard = false
        view = .empty
        questions = []
        withAnimation(Motion.open) {
            entries = []
            calls = []
        }
        drafts = [:]
        seen = []
        drawnCalls = []
        inFlightSeen = []
        seenTimers.values.forEach { $0.cancel() }
        seenTimers = [:]
        devices = []
        deviceAsked = [:]
        onView?()
    }

    func fail(_ message: String) {
        error = message
        errorTimer?.cancel()
        let timer = DispatchWorkItem { [weak self] in self?.error = nil }
        errorTimer = timer
        DispatchQueue.main.asyncAfter(deadline: .now() + 6, execute: timer)
    }

    // MARK: Actions

    func answer(_ ask: Ask, agentId: String?, optionId: String) {
        guard let agentId else { return }
        link?.request(CoreRequest.AcpPermissionReply(agentId: agentId, requestId: ask.attentionId, optionId: optionId)) {
            [weak self] result in
            if case .failure(let error) = result { self?.fail(error.message) }
        }
    }

    /// Tells the core once that this island drew the call, which a production call needs before
    /// its answer counts, retrying on transient failures like iOS's CallStore.
    func saw(_ call: PersonCall) {
        drawnCalls.insert(call.id)
        guard !seen.contains(call.id), !inFlightSeen.contains(call.id), seenTimers[call.id] == nil else { return }
        sendSeen(call.id, delays: [2, 5, 10])
    }

    private func sendSeen(_ id: String, delays: [TimeInterval]) {
        guard let link, calls.contains(where: { $0.id == id }), !seen.contains(id) else {
            inFlightSeen.remove(id)
            seenTimers.removeValue(forKey: id)?.cancel()
            return
        }
        inFlightSeen.insert(id)
        link.request(CoreCall.CallSeen(params: CallParams(id: id))) { [weak self] result in
            guard let self else { return }
            self.inFlightSeen.remove(id)
            switch result {
            case .success:
                self.seenTimers.removeValue(forKey: id)?.cancel()
                self.seen.insert(id)
            case .failure(let error):
                FileHandle.standardError.write(Data("sikemux-notch: callSeen failed for \(id): \(error.message)\n".utf8))
                guard let wait = delays.first, self.calls.contains(where: { $0.id == id }) else { return }
                let rest = Array(delays.dropFirst())
                let retry = DispatchWorkItem { [weak self] in
                    self?.seenTimers.removeValue(forKey: id)
                    self?.sendSeen(id, delays: rest)
                }
                self.seenTimers[id]?.cancel()
                self.seenTimers[id] = retry
                DispatchQueue.main.asyncAfter(deadline: .now() + wait, execute: retry)
            }
        }
    }

    /// Answers a call with what its card holds. The first answer anywhere wins.
    func answer(_ call: PersonCall, _ params: AnswerCallParams, draft: CallDraft) {
        draft.sending = true
        link?.request(CoreCall.AnswerCall(params: params)) { [weak self] result in
            draft.sending = false
            if case .failure(let error) = result { self?.fail(error.message) }
        }
    }

    /// Answers an agent's question with files, which reach it as the composer's do.
    func answer(_ question: AgentQuestion, paths: [String]) {
        link?.request(CoreCall.AnswerQuestion(params: AnswerParams(id: question.id, paths: paths))) {
            [weak self] result in
            if case .failure(let error) = result { self?.fail(error.message) }
        }
    }

    /// Lets a waiting device in with `access`, or turns it away when that is nil.
    func answer(_ device: PendingDevice, access: DeviceAccess?) {
        let request = CoreRequest.AnswerPairing(id: device.id, allow: access != nil, access: access ?? .watch)
        guard let link else { return fail(CoreLinkError.closed.message) }
        link.request(request) { [weak self] result in
            switch result {
            case .success(let answer): self?.apply(answer.status.pending)
            case .failure(let error): self?.fail(error.message)
            }
        }
    }

    /// Shows what the entry is about: its agent in Sikemux, a host's alert on Sikemux's Hosts page, or else its pull
    /// request.
    func open(_ entry: Entry) {
        if let item = entry.item, item.kind == .hostAlert {
            openHosts(item.host ?? "this")
        } else if let agentId = entry.agentId {
            focus(agentId)
        } else if let url = entry.item?.url.flatMap(URL.init(string:)) {
            NSWorkspace.shared.open(url)
        } else {
            openApp()
        }
    }

    /// Shows the agent in Sikemux. With its window closed the core keeps the
    /// agent for the window the app opens.
    func focus(_ agentId: String) {
        guard let link else { return }
        bringAppForward()
        link.request(CoreRequest.FocusAgent(agentId: agentId)) { [weak self] result in
            if case .failure = result { self?.openApp() }
        }
    }

    /// macOS refuses to bring forward an app the person did not click, so the
    /// helper, which took the click, takes the front and hands it to Sikemux.
    private func bringAppForward() {
        guard let app = options.app else { return }
        let url = URL(fileURLWithPath: app).standardizedFileURL
        guard let running = NSWorkspace.shared.runningApplications.first(where: {
            $0.bundleURL?.standardizedFileURL == url || $0.executableURL?.standardizedFileURL == url
        }) else { return }
        NSApp.activate(ignoringOtherApps: true)
        NSApp.yieldActivation(to: running)
        running.activate()
    }

    /// Opens Sikemux's Hosts page on `host`.
    func openHosts(_ host: String) {
        openLink("sikemux://hosts/", host)
    }

    /// Opens the artifact a call links on the desk that shows the call, with the call's choices at its foot.
    func openPage(_ page: CallPage, of call: PersonCall) {
        openLink("sikemux://artifact/", page.id, query: "?ask=\(call.id)")
    }

    /// Opens a link only this helper's own app follows, since Sikemux and Sikemux Dev both answer `sikemux://`.
    private func openLink(_ kind: String, _ key: String, query: String = "") {
        guard let app = options.app, app.hasSuffix(".app"),
              let key = key.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed),
              let url = URL(string: kind + key + query) else { return openApp() }
        NSWorkspace.shared.open([url], withApplicationAt: URL(fileURLWithPath: app), configuration: NSWorkspace.OpenConfiguration())
    }

    func openApp() {
        guard let app = options.app, app.hasSuffix(".app") else { return }
        NSWorkspace.shared.openApplication(at: URL(fileURLWithPath: app), configuration: .init())
    }

    struct Launch {
        let provider: String
        let project: ProjectInfo
        let yolo: Bool
        let model: String?
        let effort: String?
        let text: String
        let paths: [String]
    }

    func launcher(for provider: String) -> LauncherInfo? {
        view.workspace.launchers.first { $0.provider == provider }
    }

    var providers: [String] {
        var seen: [String] = []
        for launcher in view.workspace.launchers where !seen.contains(launcher.provider) {
            seen.append(launcher.provider)
        }
        return seen
    }

    func start(_ launch: Launch, done: @escaping (Bool) -> Void) {
        guard let link, let launcher = launcher(for: launch.provider) else {
            fail("Open Sikemux once so the notch knows which agents it can start")
            done(false)
            return
        }
        let request = CoreRequest.StartChat(
            launcher: launcher.id,
            project: launch.project.id,
            permissionMode: launch.yolo ? .bypass : .workspaceWrite,
            model: launch.model,
            effort: launch.effort
        )
        link.request(request) { [weak self] result in
            switch result {
            case .success(let answer):
                let agentId = answer.agentId
                if !launch.text.isEmpty || !launch.paths.isEmpty {
                    self?.prompt(agentId, text: launch.text, paths: launch.paths)
                }
                done(true)
            case .failure(let error):
                self?.fail(error.message)
                done(false)
            }
        }
    }

    /// Hands files to an agent: into the turn it is running, or as its next prompt.
    func send(_ paths: [String], to entry: Entry) {
        guard let agentId = entry.agentId else { return }
        let text = paths.map { ($0 as NSString).lastPathComponent }.joined(separator: ", ")
        if entry.item?.kind == .working {
            link?.request(CoreRequest.AcpSteer(agentId: agentId, text: text, paths: paths, context: [])) {
                [weak self] result in
                if case .success(let answer) = result, answer.outcome == "promptRequired" {
                    self?.prompt(agentId, text: text, paths: paths)
                }
            }
        } else {
            prompt(agentId, text: text, paths: paths)
        }
    }

    private func prompt(_ agentId: String, text: String, paths: [String]) {
        link?.request(CoreRequest.AcpPrompt(agentId: agentId, text: text, paths: paths, context: [])) {
            [weak self] result in
            if case .failure(let error) = result { self?.fail(error.message) }
        }
    }

    /// The project shown first when starting an agent: the one most of the inbox is in.
    var defaultProject: ProjectInfo? {
        let projects = view.workspace.projects
        let counts = Dictionary(grouping: entries, by: \.projectId).mapValues(\.count)
        return projects.max { (counts[$0.id] ?? 0) < (counts[$1.id] ?? 0) } ?? projects.first
    }

    /// How many calls wait on you, how many non-call items need you, how many agents work, how many subagents run across every agent, and
    /// how many items are done, leaving out what is none.
    var rollups: [(mark: Rollup, count: Int)] {
        let callIds = Set(calls.map(\.id))
        let otherNeeds = entries.filter { entry in
            guard entry.mark == .needs else { return false }
            if entry.ask != nil { return true }
            let id = entry.agent?.turn?.call?.id ?? entry.item?.questionId ?? entry.item?.hold?.id
            return id.map { !callIds.contains($0) } ?? true
        }.count
        let counts: [(Rollup, Int)] = [
            (.calls, calls.count),
            (.mark(.needs), otherNeeds),
            (.mark(.working), view.inbox.working),
            (.subagents, view.inbox.rollup?.subagents ?? 0),
            (.mark(.done), entries.filter { $0.mark == .done || $0.mark == .failed }.count),
        ]
        return counts.filter { $0.1 > 0 }.map { (mark: $0.0, count: $0.1) }
    }
}
