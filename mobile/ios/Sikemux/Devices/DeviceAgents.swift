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
        let (open, snoozed) = items()
        let teams = keptStill(teams(open))
        let folded = Folded.shared.teams
        let away = AgentTree.tree(snoozed, id: \.id, lead: \.lead, root: 0, folded: [])
        let recent = view.recent.contains { scope == nil || $0.project == scope?.id }

        MinuteClock { now in
            VStack(spacing: 0) {
                AwayNoteView()
                if teams.contains(where: { $0.project != nil || !$0.items.isEmpty }) {
                    Rows {
                        ForEach(teams, id: \.id) { team in
                            let placed = AgentTree.tree(
                                team.items, id: \.id, lead: \.lead, root: team.project == nil ? 0 : 1, folded: folded)
                            if let project = team.project {
                                ChiefRow(
                                    core: core, project: project,
                                    shown: view.inbox.agents.first { $0.chief == true && $0.project == project.id },
                                    mixed: scope == nil, now: now
                                )
                                .modifier(TreeLines(open: !placed.isEmpty))
                            }
                            ForEach(placed, id: \.item.id) { row($0, folds: true, now: now) }
                        }
                    }
                    .animation(still ? nil : .snappy(duration: 0.28), value: teams.map(\.id))
                    .onTouch { touching in
                        if !touching {
                            held = nil
                        } else if held == nil {
                            held = teams.map(\.id)
                        }
                    }
                    .padding(.top, 10)
                }
                AwayGroup(label: String(localized: "Snoozed"), fold: .snoozed, count: away.count) {
                    ForEach(away, id: \.item.id) { row($0, folds: false, now: now) }
                }
                Recent(
                    core: core, view: view, scope: scope, provider: provider, full: full, hostName: hostName, now: now)
                if open.isEmpty && snoozed.isEmpty && !recent {
                    EmptyState(
                        icon: Icons.agentLine,
                        text: scope.map { String(localized: "No agents in \($0.name).") }
                            ?? String(localized: "No agents running."))
                }
            }
        }
    }

    /// An agent the list draws: one here, or a worker an agent here started on a linked host.
    private struct Item {
        let id: String
        let lead: String?
        let project: String?
        let snoozed: Bool
        let away: Bool
        var agent: AgentShown?
        var worker: RemoteAgent?
    }

    /// The agents the list draws, each hung under its lead: every open agent and worker shown, a chief only where a
    /// list filtered to one kind has no @Chief row, and a settled one only while its team still works. A filtered list
    /// is flat, as it may have left a lead out; an agent whose lead is a chief hangs under that chief's row.
    private func items() -> (main: [Item], snoozed: [Item]) {
        let flat = provider != nil
        let chiefs = Set(view.inbox.agents.filter { $0.chief == true }.map(\.agentId))
        let leadOf = { (lead: String?) in flat ? nil : lead.flatMap { chiefs.contains($0) ? nil : $0 } }
        let here = view.inbox.agents
            .filter { listed(scope, provider, project: $0.project, type: $0.cli) && (flat || $0.chief != true) }
            .map {
                Item(
                    id: $0.agentId, lead: leadOf($0.lead), project: $0.project, snoozed: $0.snoozedUntil != nil,
                    away: $0.snoozedUntil != nil || $0.settledAt != nil, agent: $0)
            }
        let seen = Set(here.map(\.id))
        let workers = view.inbox.remoteAgents
            .filter { !seen.contains($0.agentId) && listed(nil, provider, project: nil, type: $0.cli) }
            .map {
                Item(
                    id: $0.agentId, lead: leadOf($0.lead), project: $0.project, snoozed: $0.snoozedUntil != nil,
                    away: $0.snoozedUntil != nil || $0.settledAt != nil, worker: $0)
            }
        var items = here + workers
        if let scope {
            let lead = leadsOf(items)
            let ids = Set(items.map(\.id))
            items = items.filter {
                $0.worker == nil || $0.project == scope.id
                    || seen.contains(AgentTree.top(of: $0.id, lead, among: ids))
            }
        }
        let leading = AgentTree.above(leadsOf(items), from: items.filter { !$0.away }.map(\.id))
        return (
            items.filter { !$0.away || leading.contains($0.id) },
            items.filter { $0.snoozed && !leading.contains($0.id) }
        )
    }

    private func leadsOf(_ items: [Item]) -> [String: String] {
        Dictionary(
            items.compactMap { item in item.lead.map { (item.id, $0) } }, uniquingKeysWith: { first, _ in first })
    }

    /// One project's chief and the agents under it; agents of no project shown go last with no chief.
    private struct Team {
        let project: ProjectInfo?
        let items: [Item]
        var id: String { project?.id ?? "" }
    }

    /// The list as teams. In one project, its chief leads it. With every project shown, each project with a chief or
    /// agents is a team, ordered by how the core says each stands: needs you, then at work, then the rest, ties in the
    /// host's order of projects; an agent goes with the project of the agent at the top of its tree. A list filtered
    /// to one kind of agent has no chiefs.
    private func teams(_ open: [Item]) -> [Team] {
        if provider != nil { return [Team(project: nil, items: open)] }
        if let scope { return [Team(project: scope, items: open)] }
        let shown = view.workspace.projects.filter { $0.chief != nil || $0.chiefHome != nil || ($0.agents ?? 0) > 0 }
        let ranked = shown.enumerated().sorted { (rank($0.element), $0.offset) < (rank($1.element), $1.offset) }
        let lead = leadsOf(open)
        let ids = Set(open.map(\.id))
        let projectOf = Dictionary(open.map { ($0.id, $0.project) }, uniquingKeysWith: { first, _ in first })
        let home = { (item: Item) in projectOf[AgentTree.top(of: item.id, lead, among: ids)] ?? nil }
        var teams = ranked.map { project in
            Team(project: project.element, items: open.filter { home($0) == project.element.id })
        }
        let placed = Set(shown.map(\.id))
        let loose = open.filter { item in home(item).map { !placed.contains($0) } ?? true }
        if !loose.isEmpty { teams.append(Team(project: nil, items: loose)) }
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

    /// Where a project goes in the list by how the core says it stands, as the Mac and the notch order it.
    private func rank(_ project: ProjectInfo) -> Int {
        switch project.standing {
        case .needsYou: 0
        case .atWork: 1
        default: 2
        }
    }

    private func name(_ id: String?) -> String? {
        guard let id else { return nil }
        return view.inbox.agents.first { $0.agentId == id }?.name
            ?? view.inbox.remoteAgents.first { $0.agentId == id }?.name
    }

    @ViewBuilder
    private func row(_ placed: AgentTree.Placed<Item>, folds: Bool, now: Date) -> some View {
        let item = placed.item
        let lead = name(placed.leadApart)
        let fold = folds && placed.below > 0 ? placed.below : nil
        Group {
            if let worker = item.worker {
                WorkerRow(worker: worker, now: now, lead: lead, fold: fold)
            } else if let agent = item.agent {
                let onHold = onHold.map { hold in { hold(agent.agentId) } }
                if agent.session == nil {
                    ChatRow(core: core, shown: agent, now: now, onHold: onHold, lead: lead, fold: fold)
                } else {
                    TerminalAgentRow(
                        shown: agent, now: now, onOpen: { onTab(.terminals) }, onHold: onHold, lead: lead, fold: fold)
                }
            }
        }
        .modifier(TreeLines(depth: placed.depth, lines: placed.lines, open: placed.open))
    }
}

private struct ChatRow: View {
    @Environment(\.palette) private var palette
    let core: String
    let shown: AgentShown
    let now: Date
    let onHold: (() -> Void)?
    var lead: String?
    /// How many hang below it, when it has a team to fold.
    var fold: Int?

    var body: some View {
        if let fold {
            row { TreeFold(agentId: shown.agentId, count: fold) }
        } else {
            row { EmptyView() }
        }
    }

    private func row<After: View>(@ViewBuilder after: () -> After) -> some View {
        let away = shown.snoozedUntil != nil
        let dim = away || shown.settledAt != nil
        return Row(
            title: shown.title, name: away ? nil : shown.name, lead: lead, dim: dim,
            onPress: { Router.shared.push(.chat(core: core, agent: shown.agentId)) },
            onLongPress: onHold,
            mark: {
                AskerMark(
                    agentId: shown.agentId, provider: shown.cli, chief: shown.chief == true, turn: shown.turn
                )
                .opacity(dim ? 0.55 : 1)
            },
            end: {
                if let until = shown.snoozedUntil.map(Format.date(millis:)) {
                    WakeTime(at: until, now: now)
                } else {
                    if shown.subagents > 0 { SubagentCount(count: shown.subagents) }
                    if shown.tasks > 0 { ShellCount(count: shown.tasks) }
                    if let line = shown.line { LinePlace(line: line) }
                    TurnMark(turn: shown.turn, now: now)
                }
            },
            after: after)
    }
}

/// An agent running in a terminal, listed with the chats as the host's rail lists it.
private struct TerminalAgentRow: View {
    let shown: AgentShown
    let now: Date
    let onOpen: () -> Void
    let onHold: (() -> Void)?
    var lead: String?
    var fold: Int?

    var body: some View {
        if let fold {
            row { TreeFold(agentId: shown.agentId, count: fold) }
        } else {
            row { EmptyView() }
        }
    }

    private func row<After: View>(@ViewBuilder after: () -> After) -> some View {
        let away = shown.snoozedUntil != nil
        let dim = away || shown.settledAt != nil
        return Row(
            title: shown.title, name: away ? nil : shown.name, lead: lead, dim: dim,
            onPress: onOpen, onLongPress: onHold,
            mark: {
                AskerMark(
                    agentId: shown.agentId, provider: shown.cli, chief: shown.chief == true, turn: shown.turn
                )
                .opacity(dim ? 0.55 : 1)
            },
            end: {
                if let until = shown.snoozedUntil.map(Format.date(millis:)) {
                    WakeTime(at: until, now: now)
                } else {
                    TurnMark(turn: shown.turn, now: now)
                }
            },
            after: after)
    }
}

/// Snoozed under the main list, folding on a tap of its label as the Mac's rail does.
private struct AwayGroup<Content: View>: View {
    @Environment(\.palette) private var palette
    let label: String
    let fold: Folded.Group
    let count: Int
    @ViewBuilder var rows: Content

    var body: some View {
        let folded = Folded.shared.shut.contains(fold)
        if count > 0 {
            Button {
                Folded.shared.toggle(fold)
            } label: {
                SectionLabel(text: label, count: count) {
                    IconView(icon: Icons.chevron, size: 12)
                        .foregroundStyle(palette.tertiary)
                        .rotationEffect(.degrees(folded ? 0 : 90))
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel(Text(verbatim: "\(label), \(count)"))
            .accessibilityValue(folded ? "Folded" : "Open")
            if !folded {
                Rows { rows }
            }
        }
    }
}

/// A worker an agent here started on a linked host: the row any agent has, hung under the agent that started it,
/// with the host it runs on tagged. Tapping opens its chat on that host when this phone is paired with it, and
/// otherwise says where it runs.
struct WorkerRow: View {
    @Environment(\.palette) private var palette
    let worker: RemoteAgent
    let now: Date
    var lead: String?
    /// How many hang below it, when it has a team to fold.
    var fold: Int?
    @State private var told = false

    var body: some View {
        let paired = Hub.shared.device(worker.hostId) != nil
        VStack(alignment: .leading, spacing: 0) {
            if let fold {
                row(paired: paired) { TreeFold(agentId: worker.agentId, count: fold) }
            } else {
                row(paired: paired) { EmptyView() }
            }
            if told && !paired {
                let who = worker.name.map { "@\($0)" } ?? worker.title
                Text("\(who) runs on \(worker.host). Pair this phone with \(worker.host) to open its chat.")
                    .font(Theme.ui(12.5))
                    .foregroundStyle(palette.tertiary)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.leading, 38)
                    .padding(.trailing, 8)
                    .padding(.bottom, 8)
            }
        }
    }

    private func row<After: View>(paired: Bool, @ViewBuilder after: () -> After) -> some View {
        let reachable = worker.reachable
        return Row(
            title: worker.titled == true ? worker.title : nil, name: worker.name, lead: lead,
            host: reachable ? worker.host : "\(worker.host) · \(String(localized: "Out of reach").lowercased())",
            dim: !reachable,
            onPress: {
                if paired {
                    Router.shared.push(.chat(core: worker.hostId, agent: worker.agentId))
                } else {
                    told.toggle()
                }
            },
            mark: {
                AskerMark(agentId: worker.agentId, provider: worker.cli, turn: worker.turn)
                    .opacity(reachable ? 1 : 0.55)
            },
            end: {
                if reachable {
                    if worker.subagents > 0 { SubagentCount(count: worker.subagents) }
                    if worker.tasks > 0 { ShellCount(count: worker.tasks) }
                    if let line = worker.line { LinePlace(line: line) }
                    TurnMark(turn: worker.turn, now: now)
                }
            },
            after: after)
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
            note = (String(localized: "This phone can only watch \(hostName)."), false)
            return
        }
        let live = Hub.shared.live(core)
        guard live.isOpen, let connection = live.connection else {
            Haptics.failure()
            note = (String(localized: "Not connected to the host."), true)
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
            let project = view.project(id: session.project)?.name ?? String(localized: "Other")
            if groups[project] == nil { order.append(project) }
            groups[project, default: []].append(session)
        }
        return VStack(spacing: 0) {
            if order.isEmpty {
                EmptyState(
                    icon: Icons.terminal,
                    text: scope.map { String(localized: "No terminals in \($0.name).") }
                        ?? String(localized: "No terminals open."))
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
        return session.agentType ?? String(localized: "Terminal")
    }

    private var detail: String {
        if let task = session.task { return task.command }
        if session.running { return String(localized: "Running") }
        if let signal = session.exit?.signal { return String(localized: "stopped by \(signal)") }
        return session.exit?.code.map { String(localized: "exited \($0)") } ?? String(localized: "exited")
    }
}
