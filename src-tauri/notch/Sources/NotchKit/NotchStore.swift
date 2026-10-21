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

    @ObservationIgnored let options: Options
    @ObservationIgnored private var link: CoreLink?
    @ObservationIgnored private var startedCore = false
    @ObservationIgnored private var waiter: CoreWaiter?
    @ObservationIgnored private var questions: [AgentQuestion] = []
    @ObservationIgnored private var heard = false
    @ObservationIgnored private var errorTimer: DispatchWorkItem?
    @ObservationIgnored var onPeek: ((Peek) -> Void)?

    enum Peek {
        case ask(String)
        case done(String)
        case connect(String)
    }

    init(options: Options) {
        self.options = options
        settings = NotchSettings.load(options.settings)
        Haptics.enabled = settings.haptics
    }

    func reloadSettings() {
        let fresh = NotchSettings.load(options.settings)
        Haptics.enabled = fresh.haptics
        if fresh != settings { settings = fresh }
    }

    // MARK: Connection

    func connect() {
        guard link == nil, waitForCore() else { return }
        let link = CoreLink(socket: options.socket, protocolVersion: options.protocolVersion)
        link.onReady = { [weak self, weak link] in
            link?.request(["op": "watchView"])
            link?.request(["op": "remoteStatus"]) { result in
                if case .success(let response) = result { self?.receiveRemote(response["status"]) }
            }
            self?.error = nil
        }
        link.onEvent = { [weak self] event in self?.receive(event) }
        link.onClose = { [weak self] reason in
            guard let self else { return }
            self.link = nil
            self.disconnected()
            guard let reason else { return self.connect() }
            self.fail(reason)
            _ = self.waitForCore()
        }
        if link.connect() {
            self.link = link
            waiter = nil
            return
        }
        if !startedCore {
            startedCore = true
            CoreStarter.start(options) { [weak self] in self?.connect() }
        }
    }

    /// Listens for the core starting, stopping or opening a window. It listens
    /// before each try, so a core that comes up during one still wakes it.
    private func waitForCore() -> Bool {
        if waiter != nil { return true }
        waiter = CoreWaiter(socket: options.socket) { [weak self] in self?.connect() }
        if waiter == nil { fail("the notch could not wait for Sikemux's core at \(options.socket)") }
        return waiter != nil
    }

    private func receive(_ event: [String: Any]) {
        if event["kind"] as? String == "remote" {
            receiveRemote(event["status"])
            return
        }
        guard event["kind"] as? String == "deviceView", let raw = event["view"],
              let data = try? JSONSerialization.data(withJSONObject: raw),
              let view = try? JSONDecoder().decode(DeviceView.self, from: data)
        else { return }
        apply(view)
    }

    func apply(_ view: DeviceView) {
        let before = Dictionary(entries.map { ($0.id, $0.item.kind) }, uniquingKeysWith: { first, _ in first })
        self.view = view
        withAnimation(Motion.open) { entries = view.entries(questions) }
        if !Set(view.inbox.items.compactMap(\.questionId)).isSubset(of: questions.map(\.id)) { readQuestions() }
        defer { heard = true }
        guard heard else { return }
        let items = view.inbox.items
        if let ask = items.last(where: { $0.tier == .needs && before[$0.id] == nil }) {
            onPeek?(.ask(ask.id))
        } else if let done = items.last(where: { before[$0.id] == .working && $0.kind != .working }) {
            onPeek?(.done(done.id))
        }
    }

    private func readQuestions() {
        link?.request(["op": "call", "method": "questions", "params": [String: Any]()]) { [weak self] result in
            guard case .success(let response) = result, let raw = response["result"],
                  let data = try? JSONSerialization.data(withJSONObject: raw),
                  let questions = try? JSONDecoder().decode([AgentQuestion].self, from: data)
            else { return }
            self?.apply(questions)
        }
    }

    func apply(_ questions: [AgentQuestion]) {
        guard questions != self.questions else { return }
        self.questions = questions
        withAnimation(Motion.open) { entries = view.entries(questions) }
    }

    private func receiveRemote(_ raw: Any?) {
        guard let raw, let data = try? JSONSerialization.data(withJSONObject: raw),
              let status = try? JSONDecoder().decode(RemoteStatus.self, from: data)
        else { return }
        apply(status.pending)
    }

    /// Unlike an agent's request, a device already waiting when the notch connects still opens the island.
    func apply(_ pending: [PendingDevice], now: Date = Date()) {
        let fresh = pending.filter { deviceAsked[$0.id] == nil }
        deviceAsked = Dictionary(pending.map { ($0.id, deviceAsked[$0.id] ?? now) }, uniquingKeysWith: { first, _ in first })
        if pending != devices { devices = pending }
        if !fresh.isEmpty, let oldest = pending.first { onPeek?(.connect(oldest.id)) }
    }

    /// Empties the island while the core is away. The first view after it comes
    /// back only catches up, so what was already asked or finished does not peek again.
    private func disconnected() {
        heard = false
        view = .empty
        questions = []
        withAnimation(Motion.open) { entries = [] }
        devices = []
        deviceAsked = [:]
    }

    /// Shows what went wrong for a few seconds.
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
        link?.request([
            "op": "acpPermissionReply",
            "agentId": agentId,
            "requestId": ask.attentionId,
            "optionId": optionId,
        ]) { [weak self] result in
            if case .failure(let error) = result { self?.fail(error.message) }
        }
    }

    /// Answers an agent's question with one of the answers it offered. The first answer anywhere wins.
    func answer(_ question: AgentQuestion, choice: Int) {
        link?.request(["op": "call", "method": "answerQuestion", "params": ["id": question.id, "choice": choice]]) {
            [weak self] result in
            if case .failure(let error) = result { self?.fail(error.message) }
        }
    }

    /// Lets a waiting device in with `access`, or turns it away when that is nil.
    func answer(_ device: PendingDevice, access: DeviceAccess?) {
        let request: [String: Any] = [
            "op": "answerPairing",
            "id": device.id,
            "allow": access != nil,
            "access": (access ?? .watch).rawValue,
        ]
        guard let link else { return fail(CoreLinkError.closed.message) }
        link.request(request) { [weak self] result in
            switch result {
            case .success(let response): self?.receiveRemote(response["status"])
            case .failure(let error): self?.fail(error.message)
            }
        }
    }

    /// Shows what the entry is about: its agent in Sikemux, or else its pull request.
    func open(_ entry: Entry) {
        if let agentId = entry.item.agentId {
            focus(agentId)
        } else if let url = entry.item.url.flatMap(URL.init(string:)) {
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
        link.request(["op": "focusAgent", "agentId": agentId]) { [weak self] result in
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
        var request: [String: Any] = [
            "op": "startChat",
            "launcher": launcher.id,
            "project": launch.project.id,
            "permissionMode": launch.yolo ? "bypass" : "workspace-write",
        ]
        if let model = launch.model { request["model"] = model }
        if let effort = launch.effort { request["effort"] = effort }
        link.request(request) { [weak self] result in
            switch result {
            case .success(let response):
                guard let agentId = response["agentId"] as? String else { return done(false) }
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
        guard let agentId = entry.item.agentId else { return }
        let text = paths.map { ($0 as NSString).lastPathComponent }.joined(separator: ", ")
        if entry.item.kind == .working {
            link?.request(["op": "acpSteer", "agentId": agentId, "text": text, "paths": paths, "context": [Any]()]) {
                [weak self] result in
                if case .success(let response) = result, response["outcome"] as? String == "promptRequired" {
                    self?.prompt(agentId, text: text, paths: paths)
                }
            }
        } else {
            prompt(agentId, text: text, paths: paths)
        }
    }

    private func prompt(_ agentId: String, text: String, paths: [String]) {
        link?.request(["op": "acpPrompt", "agentId": agentId, "text": text, "paths": paths, "context": [Any]()]) {
            [weak self] result in
            if case .failure(let error) = result { self?.fail(error.message) }
        }
    }

    /// The project shown first when starting an agent: the one most of the inbox is in.
    var defaultProject: ProjectInfo? {
        let projects = view.workspace.projects
        let counts = Dictionary(grouping: entries, by: \.project).mapValues(\.count)
        return projects.max { (counts[$0.name] ?? 0) < (counts[$1.name] ?? 0) } ?? projects.first
    }

    /// How many items need you, how many agents work, how many subagents and shells run across every agent, and
    /// how many items are done, leaving out what is none.
    var rollups: [(mark: Rollup, count: Int)] {
        let counts: [(Rollup, Int)] = [
            (.mark(.needs), entries.filter { $0.mark == .needs }.count),
            (.mark(.working), view.inbox.working),
            (.subagents, view.inbox.agents.reduce(0) { $0 + $1.subagents }),
            (.shells, view.inbox.agents.reduce(0) { $0 + $1.tasks }),
            (.mark(.done), entries.filter { $0.mark == .done || $0.mark == .failed }.count),
        ]
        return counts.filter { $0.1 > 0 }.map { (mark: $0.0, count: $0.1) }
    }
}
