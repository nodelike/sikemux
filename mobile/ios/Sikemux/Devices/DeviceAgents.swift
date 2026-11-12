import SikemuxCore
import SwiftUI

/// The host's agents: its open agents in one list, each with its own state, then its recent chats.
struct DeviceAgents: View {
    @Environment(\.palette) private var palette
    let core: String
    let view: DeviceView
    let scope: ProjectInfo?
    let provider: String?
    let full: Bool
    let hostName: String
    let onTab: (Route.DeviceTab) -> Void
    /// Opens an agent's snooze sheet; absent for a phone that can only watch.
    let onHold: ((String) -> Void)?

    @Environment(\.accessibilityReduceMotion) private var still
    /// The projects' order while a finger is on the list, so a tap never lands on a row that moved under it.
    @State private var held: [String]?

    var body: some View {
        // The open agents in the order the host lists them on every device: the main list, what wants the person
        // first, then the snoozed and then the settled ones.
        let agents = view.inbox.agents.filter { listed(scope, provider, project: $0.project, type: $0.cli) }
        let open = agents.filter { agent in
            agent.snoozedUntil == nil && agent.settledAt == nil && (provider != nil || agent.chief != true)
        }
        let teams = keptStill(teams(open))
        let away = agents.filter { provider != nil || $0.chief != true }
        let snoozed = away.filter { $0.snoozedUntil != nil && $0.settledAt == nil }
        let settled = away.filter { $0.settledAt != nil }
        let workers = view.inbox.remoteAgents
        let leads = Set(open.map(\.agentId))
        let strays =
            scope != nil
            ? []
            : workers.filter { worker in
                !(worker.lead.map(leads.contains) ?? false) && listed(nil, provider, project: nil, type: worker.cli)
            }
        let recent = view.recent.contains { scope == nil || $0.project == scope?.id }

        MinuteClock { now in
            VStack(spacing: 0) {
                AwayNoteView()
                if teams.contains(where: { $0.project != nil || !$0.agents.isEmpty }) || !strays.isEmpty {
                    Rows {
                        ForEach(teams, id: \.id) { team in
                            if let project = team.project {
                                ChiefRow(
                                    core: core, project: project,
                                    shown: view.inbox.agents.first { $0.chief == true && $0.project == project.id },
                                    mixed: scope == nil, now: now)
                            }
                            ForEach(team.agents, id: \.agentId) { agent in
                                row(agent, now: now)
                                ForEach(workers.filter { $0.lead == agent.agentId }, id: \.agentId) {
                                    WorkerRow(worker: $0, nested: true)
                                }
                            }
                        }
                        ForEach(strays, id: \.agentId) { WorkerRow(worker: $0) }
                    }
                    .animation(still ? nil : .snappy(duration: 0.28), value: teams.map(\.id))
                    .simultaneousGesture(
                        DragGesture(minimumDistance: 0)
                            .onChanged { _ in if held == nil { held = teams.map(\.id) } }
                            .onEnded { _ in held = nil }
                    )
                    .padding(.top, 10)
                }
                AwayGroup(label: "Snoozed", fold: .snoozed, agents: snoozed) { row($0, now: now) }
                AwayGroup(label: "Settled", fold: .settled, agents: settled) { row($0, now: now) }
                Recent(
                    core: core, view: view, scope: scope, provider: provider, full: full, hostName: hostName, now: now)
                if open.isEmpty && strays.isEmpty && snoozed.isEmpty && settled.isEmpty && !recent {
                    EmptyState(
                        icon: Icons.agentLine, text: scope.map { "No agents in \($0.name)." } ?? "No agents running.")
                }
            }
        }
    }

    /// One project's chief and the agents under it; agents of no project shown go last with no chief.
    private struct Team {
        let project: ProjectInfo?
        let agents: [AgentShown]
        var id: String { project?.id ?? "" }
    }

    /// The list as teams. In one project, its chief leads it. With every project shown, each project with a chief or
    /// agents is a team, those that most need the person first: an open call or an agent that needs you, then work
    /// under way, then the rest, ties in the host's order of projects. A list filtered to one kind of agent has no
    /// chiefs.
    private func teams(_ open: [AgentShown]) -> [Team] {
        if provider != nil { return [Team(project: nil, agents: open)] }
        if let scope { return [Team(project: scope, agents: open)] }
        let shown = view.workspace.projects.filter { $0.chief != nil || ($0.agents ?? 0) > 0 }
        let ranked = shown.enumerated().sorted { (need($0.element), $0.offset) < (need($1.element), $1.offset) }
        var teams = ranked.map { project in
            Team(project: project.element, agents: open.filter { $0.project == project.element.id })
        }
        let placed = Set(shown.map(\.id))
        let loose = open.filter { agent in agent.project.map { !placed.contains($0) } ?? true }
        if !loose.isEmpty { teams.append(Team(project: nil, agents: loose)) }
        return teams
    }

    /// The teams in the order they had when a finger came down, while it stays; new ones after them.
    private func keptStill(_ teams: [Team]) -> [Team] {
        guard let held else { return teams }
        let at = Dictionary(held.enumerated().map { ($1, $0) }, uniquingKeysWith: { first, _ in first })
        return teams.enumerated()
            .sorted { (at[$0.element.id] ?? held.count + $0.offset) < (at[$1.element.id] ?? held.count + $1.offset) }
            .map(\.element)
    }

    /// How much a project needs the person: 0 with an open call or an agent that needs them, 1 with work under way,
    /// else 2. The Mac and the notch order projects by the same.
    private func need(_ project: ProjectInfo) -> Int {
        let rows = view.inbox.agents.filter { $0.project == project.id }
        let called = (view.calls ?? []).contains { $0.state == .open && $0.project == project.path }
        if called || rows.contains(where: { $0.turn?.state == .needsYou }) { return 0 }
        if rows.contains(where: { $0.turn?.state == .working || $0.turn?.state == .waiting }) { return 1 }
        return 2
    }

    @ViewBuilder
    private func row(_ agent: AgentShown, now: Date) -> some View {
        let onHold = onHold.map { hold in { hold(agent.agentId) } }
        // A filtered list may have left a teammate's lead out, so it hangs from nothing.
        let team = provider == nil && agent.snoozedUntil == nil && agent.settledAt == nil ? agent.team : nil
        Group {
            if agent.session == nil {
                ChatRow(core: core, shown: agent, now: now, onHold: onHold)
            } else {
                TerminalAgentRow(shown: agent, now: now, onOpen: { onTab(.terminals) }, onHold: onHold)
            }
        }
        .modifier(TeamLines(team: team))
    }
}

private struct ChatRow: View {
    @Environment(\.palette) private var palette
    let core: String
    let shown: AgentShown
    let now: Date
    let onHold: (() -> Void)?

    var body: some View {
        let away = shown.snoozedUntil != nil || shown.settledAt != nil
        Row(
            title: shown.title, name: away ? nil : shown.name,
            detail: shown.settledAt.map { Format.ago(Format.date(millis: $0), now: now) },
            dim: away,
            onPress: { Router.shared.push(.chat(core: core, agent: shown.agentId)) },
            onLongPress: onHold,
            mark: {
                AskerMark(
                    agentId: shown.agentId, provider: shown.cli, chief: shown.chief == true, turn: shown.turn?.state
                )
                .opacity(away ? 0.55 : 1)
            },
            end: {
                if away {
                    if shown.settledAt == nil, let until = shown.snoozedUntil.map(Format.date(millis:)) {
                        WakeTime(at: until, now: now)
                    }
                } else {
                    if shown.subagents > 0 { SubagentCount(count: shown.subagents) }
                    if shown.tasks > 0 { ShellCount(count: shown.tasks) }
                    TurnMark(turn: shown.turn, now: now)
                }
            },
            after: {
                if shown.settledAt != nil, onHold != nil { RestoreButton(core: core, shown: shown) }
            })
    }
}

/// An agent running in a terminal, listed with the chats as the host's rail lists it.
private struct TerminalAgentRow: View {
    let shown: AgentShown
    let now: Date
    let onOpen: () -> Void
    let onHold: (() -> Void)?

    var body: some View {
        let snoozed = shown.snoozedUntil != nil && shown.settledAt == nil
        let away = snoozed || shown.settledAt != nil
        Row(
            title: shown.title, name: away ? nil : shown.name, dim: away,
            onPress: onOpen, onLongPress: onHold,
            mark: {
                AskerMark(
                    agentId: shown.agentId, provider: shown.cli, chief: shown.chief == true, turn: shown.turn?.state
                )
                .opacity(away ? 0.55 : 1)
            },
            end: {
                if snoozed, let until = shown.snoozedUntil.map(Format.date(millis:)) {
                    WakeTime(at: until, now: now)
                } else if !away {
                    TurnMark(turn: shown.turn, now: now)
                }
            })
    }
}

/// The spine and tick that hang a teammate's row under its lead, as the Mac's rail draws them.
private struct TeamLines: ViewModifier {
    @Environment(\.palette) private var palette
    let team: TeamLink?

    func body(content: Content) -> some View {
        let member = team == .member || team == .last
        content
            .padding(.leading, member ? 22 : 0)
            .background(alignment: .topLeading) {
                if let team { TeamPath(team: team).stroke(palette.treeSpine, lineWidth: 1) }
            }
    }
}

private struct TeamPath: Shape {
    let team: TeamLink
    /// Down the middle of the lead's mark, which sits 8pt into its row.
    private let spine: CGFloat = 18.5
    /// Half a row, where a teammate's mark is centred, and the gap between rows.
    private let middle: CGFloat = 24
    private let gap: CGFloat = 2

    func path(in rect: CGRect) -> Path {
        var path = Path()
        switch team {
        case .head:
            path.move(to: CGPoint(x: spine, y: middle + 12))
            path.addLine(to: CGPoint(x: spine, y: rect.maxY + gap))
        case .member:
            path.move(to: CGPoint(x: spine, y: -gap))
            path.addLine(to: CGPoint(x: spine, y: rect.maxY + gap))
            path.move(to: CGPoint(x: spine, y: middle))
            path.addLine(to: CGPoint(x: spine + 8, y: middle))
        case .last:
            path.move(to: CGPoint(x: spine, y: -gap))
            path.addLine(to: CGPoint(x: spine, y: middle - 4))
            path.addQuadCurve(to: CGPoint(x: spine + 4, y: middle), control: CGPoint(x: spine, y: middle))
            path.addLine(to: CGPoint(x: spine + 8, y: middle))
        case .unknown:
            break
        }
        return path
    }
}

/// Puts a settled agent back in the main list.
private struct RestoreButton: View {
    @Environment(\.palette) private var palette
    let core: String
    let shown: AgentShown
    @State private var failure: Failure?

    var body: some View {
        if let connection = Hub.shared.live(core).connection, Hub.shared.live(core).isOpen {
            Button {
                Haptics.tap()
                Task {
                    do {
                        try await AwaySheet.settle(connection, [shown.agentId], false)
                    } catch {
                        Haptics.failure()
                        failure = Failure("Couldn't restore it", error)
                    }
                }
            } label: {
                Text("Restore")
                    .font(Theme.ui(12.5, .medium))
                    .foregroundStyle(palette.ink)
                    .padding(.horizontal, 10)
                    .frame(height: 28)
                    .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(palette.raised))
                    .overlay(
                        RoundedRectangle(cornerRadius: 8, style: .continuous).strokeBorder(palette.border, lineWidth: 1)
                    )
            }
            .buttonStyle(PressFade())
            .padding(.leading, 6)
            .accessibilityLabel("Restore \(shown.title)")
            .failureAlert($failure)
        }
    }
}

/// Snoozed or Settled under the main list, folding on a tap of its label as the Mac's rail does.
private struct AwayGroup<Content: View>: View {
    @Environment(\.palette) private var palette
    let label: String
    let fold: Folded.Group
    let agents: [AgentShown]
    @ViewBuilder var row: (AgentShown) -> Content

    var body: some View {
        let folded = Folded.shared.shut.contains(fold)
        if !agents.isEmpty {
            Button {
                Folded.shared.toggle(fold)
            } label: {
                SectionLabel(text: label, count: agents.count) {
                    IconView(icon: Icons.chevron, size: 12)
                        .foregroundStyle(palette.tertiary)
                        .rotationEffect(.degrees(folded ? 0 : 90))
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("\(label), \(agents.count)")
            .accessibilityValue(folded ? "Folded" : "Open")
            if !folded {
                Rows { ForEach(agents, id: \.agentId) { row($0) } }
            }
        }
    }
}

/// A worker an agent here started on a linked host, set in when it sits under the agent that started it.
struct WorkerRow: View {
    @Environment(\.palette) private var palette
    let worker: RemoteAgent
    var nested = false

    var body: some View {
        Row(
            name: worker.name, host: worker.host, detail: worker.reachable ? worker.label : "Out of reach",
            place: worker.project,
            dim: !worker.reachable,
            mark: { AgentMark(provider: worker.cli, size: 16).opacity(worker.reachable ? 1 : 0.5) },
            end: {
                if worker.reachable {
                    if let turn = worker.turn {
                        TurnMark(turn: turn, now: Date())
                    } else {
                        AgentStateMark(group: worker.group)
                    }
                }
            }
        )
        .padding(.leading, nested ? 30 : 0)
    }
}

/// Saved chats the host's rail lists as recent; a phone with full access takes one up again.
private struct Recent: View {
    @Environment(\.palette) private var palette
    let core: String
    let view: DeviceView
    let scope: ProjectInfo?
    let provider: String?
    let full: Bool
    let hostName: String
    let now: Date
    @State private var all = false
    @State private var resuming: String?
    @State private var note: (text: String, failed: Bool)?
    private static let shownFirst = 10

    var body: some View {
        let chats = view.recent.filter {
            $0.agentId == nil && listed(scope, provider, project: $0.project, type: $0.provider)
        }
        let shown = all ? chats : Array(chats.prefix(Self.shownFirst))
        if !chats.isEmpty {
            SectionLabel("Recent")
            Rows {
                ForEach(shown, id: \.id) { chat in
                    Row(
                        title: chat.title,
                        onPress: { resume(chat) },
                        mark: { AgentMark(provider: chat.provider, size: 20) },
                        end: {
                            if resuming == chat.id {
                                Working()
                            } else {
                                Text(Format.age(Format.date(millis: chat.activeAt), now: now)).font(Theme.ui(12))
                                    .monospacedDigit().foregroundStyle(palette.tertiary)
                            }
                        })
                }
                if shown.count < chats.count {
                    Button {
                        all = true
                    } label: {
                        Text("Show \(chats.count - shown.count) more")
                            .metaStyle(palette)
                            .padding(.leading, 38)
                            .padding(.trailing, 8)
                            .frame(maxWidth: .infinity, minHeight: 40, alignment: .leading)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(RowPressStyle())
                }
            }
            if let note {
                Text(note.text)
                    .font(Theme.ui(12.5))
                    .foregroundStyle(note.failed ? palette.danger : palette.tertiary)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 8)
                    .padding(.top, 8)
            }
        }
    }

    private func resume(_ chat: RecentInfo) {
        guard resuming == nil else { return }
        guard full else {
            Haptics.warning()
            note = ("This phone can only watch \(hostName).", false)
            return
        }
        let live = Hub.shared.live(core)
        guard live.isOpen, let connection = live.connection else {
            Haptics.failure()
            note = ("Not connected to the host.", true)
            return
        }
        Haptics.tap()
        note = nil
        resuming = chat.id
        Task {
            defer { resuming = nil }
            do {
                let agentId = try await connection.resumeChat(recent: chat.id)
                Router.shared.push(.chat(core: core, agent: agentId))
            } catch {
                Haptics.failure()
                note = (problem(error), true)
            }
        }
    }
}

/// The host's terminals, by project.
struct Terminals: View {
    @Environment(\.palette) private var palette
    let view: DeviceView
    let scope: ProjectInfo?

    var body: some View {
        var order: [String] = []
        var groups: [String: [SessionInfo]] = [:]
        for session in view.sessions where scope == nil || session.project == scope?.id {
            let project = view.project(id: session.project)?.name ?? "Other"
            if groups[project] == nil { order.append(project) }
            groups[project, default: []].append(session)
        }
        return VStack(spacing: 0) {
            if order.isEmpty {
                EmptyState(
                    icon: Icons.terminal, text: scope.map { "No terminals in \($0.name)." } ?? "No terminals open.")
            }
            ForEach(order, id: \.self) { project in
                if scope == nil { SectionLabel(project) } else { Color.clear.frame(height: 8) }
                Rows { ForEach(groups[project] ?? [], id: \.id) { TerminalRow(session: $0) } }
            }
        }
    }
}

private struct TerminalRow: View {
    @Environment(\.palette) private var palette
    @Environment(\.hostCore) private var core
    let session: SessionInfo

    var body: some View {
        Row(
            title: title, name: session.task == nil ? session.agentName : nil,
            detailText: Text(detail).font(Theme.mono(12)), dim: !session.running,
            mark: {
                IconView(icon: Icons.terminal, size: 15)
                    .foregroundStyle(session.running ? palette.live : palette.tertiary)
            },
            end: {
                if session.running {
                    if session.task != nil { Working() } else { Circle().fill(palette.live).frame(width: 7, height: 7) }
                }
            })
    }

    private var title: String {
        if let task = session.task { return task.label }
        if session.agentName != nil { return session.title ?? Catalog.label(session.agentType ?? "agent", core: core) }
        return session.agentType ?? "Terminal"
    }

    private var detail: String {
        if let task = session.task { return task.command }
        if session.running { return "Running" }
        if let signal = session.exit?.signal { return "stopped by \(signal)" }
        return session.exit?.code.map { "exited \($0)" } ?? "exited"
    }
}
