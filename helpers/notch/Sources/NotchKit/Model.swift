import Foundation

extension DeviceView {
    static let empty = DeviceView(
        workspace: Workspace(projects: [], launchers: [], palette: [:]), attentions: [],
        inbox: Inbox(items: [], working: 0, remoteAgents: [], agents: [], rollup: nil, teamNeedsYou: nil), calls: [], now: nil,
        faceStyle: FaceStyle(motion: true), load: nil
    )
}

enum Mark: Hashable {
    case needs, working, done, failed, idle
    /// A ring filling as a wait on CI or the checks runs, once its length is known, and the
    /// timer of any other wait.
    case waiting, timer
    /// A hollow ring: the turn stopped short.
    case stopped

    /// The mark for how an agent's turn stands, as the core sends it; none for a state this
    /// build does not know, or a finished turn the person has seen.
    init?(turn: TurnState) {
        switch turn.state {
        case .working: self = .working
        case .waiting: self = turn.wait.map(Mark.fills) == true ? .waiting : .timer
        case .needsYou: self = .needs
        case .done:
            if turn.seen == true { return nil }
            self = .done
        case .failed: self = .failed
        case .stopped: self = .stopped
        case .unknown: return nil
        }
    }

    /// Whether a wait shows the ring: one on CI or the checks whose length is known.
    static func fills(_ wait: TurnWait) -> Bool {
        (wait.kind == .ci || wait.kind == .check) && (wait.expectedMs ?? 0) > 0
    }
}

enum Rollup: Hashable {
    /// The calls waiting on the person, which stand in for needs you while there are any.
    case calls
    case mark(Mark)
    case subagents

    /// Totals show their count even at one; a state shows it only past one.
    func showsCount(_ count: Int) -> Bool {
        if case .mark = self { return count > 1 }
        return true
    }
}

struct Ask: Equatable {
    let attentionId: String
    /// The command it wants to run, or what the tool call is called when it is not a command.
    let command: String
    let isCommand: Bool
    let allowOnce: String?
    let allowAlways: String?
    let reject: String?

    init(_ attention: Attention) {
        let choices = attention.choices
        attentionId = attention.id
        command = choices.command ?? choices.title ?? "a tool"
        isCommand = choices.command != nil
        allowOnce = choices.allowOnce
        allowAlways = choices.allowAlways
        reject = choices.reject
    }
}

/// One row of the island: an open agent as the core shows it, with what the inbox says about it, or an item of an
/// agent on another host.
struct Entry: Identifiable, Equatable {
    let item: InboxItem?
    let agent: AgentShown?
    let ask: Ask?
    var question: AgentQuestion? = nil
    var host: String? = nil
    var reachable: Bool = true
    var depth: Int = 0
    var leadName: String? = nil
    var teamNeedsYou: Bool = false

    var id: String { item?.id ?? "agent:\(agent?.agentId ?? "")" }
    var agentId: String? { agent?.agentId ?? item?.agentId }
    var name: String? { item?.name ?? agent?.name }
    var lead: String? { agent?.lead }
    var provider: String { item?.cli ?? agent?.cli ?? "" }
    var project: String { item?.projectName ?? agent?.projectName ?? "" }
    /// The workspace project it is in, by id, which tells apart two projects of one name.
    var projectId: String? { agent?.project ?? item?.project }
    /// What the core says its row leads with; an agent the inbox has nothing about goes by its title.
    var title: String { item?.heading ?? (isChief ? "Chief of staff" : agent?.title) ?? "" }
    /// The attempt its agent is retrying a failed request on, 0 when it did not say.
    var reconnecting: Int? { agent?.reconnecting }
    /// Subagents, and shells or monitors, its agent still has going.
    var subagents: Int { agent?.subagents ?? 0 }
    var tasks: Int { agent?.tasks ?? 0 }
    var inLine: InLine? { agent?.line }
    /// When its agent was last named with `@` or messaged by another agent.
    var calledAt: Int? { agent?.calledAt }
    /// It is its project's chief, @Chief.
    var isChief: Bool { agent?.chief == true }
    var isChat: Bool { agentId != nil && session == nil && !provider.isEmpty }
    var session: Int? { agent?.session ?? item?.session }
    var since: Date? { item.map { Date(timeIntervalSince1970: Double($0.at) / 1000) } }

    var mark: Mark {
        if let agent {
            return agent.turn.flatMap(Mark.init(turn:)) ?? .idle
        }
        return switch item?.tier {
        case .needs: .needs
        case .progress: .working
        case .review, .finished: item?.kind == .failed ? .failed : .done
        case .unknown, nil: .idle
        }
    }

    /// What the item says, and for a question why it reached the person rather than its lead.
    var what: String {
        [item?.about == nil ? nil : item?.what, question?.viaNote]
            .compactMap { $0 }.joined(separator: " · ")
    }
}

/// What the core calls each agent, from the catalog it sends once the island connects.
enum AgentCatalog {
    static var names: [String: String] = [:]
}

func agentName(_ provider: String) -> String {
    AgentCatalog.names[provider] ?? provider.capitalized
}

extension DeviceView {
    /// The load against the cores and agents' builds while this Mac is overloaded, as its row says them.
    var busy: String? {
        guard let load, load.overloaded == true else { return nil }
        let builds = load.builds?.count ?? 0
        let average = load.loadAverage.map { "load \($0) on \(load.cores) cores" }
        let counted = builds == 0 ? nil : builds == 1 ? "1 build" : "\(builds) builds"
        return [average, counted].compactMap { $0 }.joined(separator: " · ")
    }

    /// What about a host on the account needs the person, each its own row above the agents.
    var hostAlerts: [Entry] {
        inbox.items.filter { $0.kind == .hostAlert }.map { Entry(item: $0, agent: nil, ask: nil) }
    }

    /// Every open agent in the main list once, in the order the core lists them on every device, each with what the inbox says about it
    /// and the question it asks from `questions`. Agents on other hosts the inbox lists follow; items that are not about
    /// an agent are left out.
    func entries(_ questions: [AgentQuestion]) -> [Entry] {
        let localAgents = inbox.agents.filter { $0.snoozedUntil == nil && $0.settledAt == nil }
        let hidden = Set(inbox.agents.map(\.agentId)).subtracting(localAgents.map(\.agentId))
        let remoteById = Dictionary(
            inbox.remoteAgents
                .filter { $0.snoozedUntil == nil && $0.settledAt == nil && !hidden.contains($0.agentId) }
                .map { ($0.agentId, $0) },
            uniquingKeysWith: { first, _ in first }
        )
        let localIds = Set(localAgents.map(\.agentId))
        let remotes = inbox.remoteAgents
            .filter { $0.snoozedUntil == nil && $0.settledAt == nil && !localIds.contains($0.agentId) && !hidden.contains($0.agentId) }
            .map { $0.shown }
        let agents = localAgents + remotes
        let listed = Set(agents.map(\.agentId))
        var seen = Set<String>()
        let others = inbox.items.compactMap(\.agentId)
            .filter { !listed.contains($0) && !hidden.contains($0) && seen.insert($0).inserted }
        let teamNeeds = Set(inbox.teamNeedsYou ?? [])
        let projectsById = Dictionary(workspace.projects.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        let entries = (agents.map(\.agentId) + others).map { agentId in
            let item = inbox.items.first { $0.agentId == agentId }
            let agent = agents.first { $0.agentId == agentId }
            let remote = remoteById[agentId]
            let projectId = agent?.project ?? item?.project
            let chiefHome = (agent?.chief == true) ? projectId.flatMap { projectsById[$0]?.chiefHome } : nil
            let host = remote?.host ?? item?.host ?? chiefHome?.hostName
            let reachable = remote?.reachable ?? (chiefHome.map { $0.awaySince == nil } ?? true)
            let needsTeam = (agent?.chief == true) && projectId.map { teamNeeds.contains($0) } == true
            return Entry(
                item: item,
                agent: agent,
                ask: attentions.first { $0.id == item?.requestId }.map(Ask.init),
                question: questions.first { $0.id == item?.questionId },
                host: host,
                reachable: reachable,
                teamNeedsYou: needsTeam
            )
        }
        return byProject(entries)
    }

    /// The entries project by project, each project's @Chief first and its agents after it in
    /// lead-tree order, projects in the order of how the core says they stand: needs you, then at
    /// work, then the rest, each in the order the core lists them. Entries of no listed project
    /// follow, as they were.
    func byProject(_ entries: [Entry]) -> [Entry] {
        let projects = workspace.projects
        let namesById = Dictionary(
            entries.compactMap { entry -> (String, String)? in
                guard let id = entry.agentId, let name = entry.name else { return nil }
                return (id, name)
            },
            uniquingKeysWith: { first, _ in first }
        )
        func belongs(_ entry: Entry, _ project: ProjectInfo) -> Bool { entry.projectId == project.id }
        func rank(_ project: ProjectInfo) -> Int {
            switch project.standing {
            case .needsYou: 0
            case .atWork: 1
            default: 2
            }
        }
        let ranked = projects.enumerated()
            .filter { _, project in entries.contains { belongs($0, project) } }
            .sorted { (rank($0.element), $0.offset) < (rank($1.element), $1.offset) }
            .map(\.element)
        var ordered: [Entry] = []
        for project in ranked {
            let mine = entries.filter { belongs($0, project) }
            let chiefs = mine.filter(\.isChief)
            let workers = Self.withHierarchy(mine.filter { !$0.isChief }, rootDepth: chiefs.isEmpty ? 0 : 1, namesById: namesById)
            ordered += chiefs + workers
        }
        let placed = Set(ordered.map(\.id))
        let loose = Self.withHierarchy(entries.filter { !placed.contains($0.id) }, rootDepth: 0, namesById: namesById)
        return ordered + loose
    }

    private static let maxDepth = 3

    private static func withHierarchy(_ rows: [Entry], rootDepth: Int, namesById: [String: String]) -> [Entry] {
        let indexById = Dictionary(
            rows.enumerated().compactMap { ix, row in row.agentId.map { ($0, ix) } },
            uniquingKeysWith: { first, _ in first }
        )
        let parent: [Int?] = rows.enumerated().map { ix, row in
            guard let lead = row.lead, let parentIx = indexById[lead], parentIx != ix else { return nil }
            return parentIx
        }
        var children = Array(repeating: [Int](), count: rows.count)
        for (ix, lead) in parent.enumerated() {
            if let lead { children[lead].append(ix) }
        }
        var placed = Array(repeating: false, count: rows.count)
        var order: [(Int, Int)] = []
        func walk(_ ix: Int, _ depth: Int) {
            placed[ix] = true
            order.append((ix, depth))
            for child in children[ix] where !placed[child] {
                walk(child, depth + 1)
            }
        }
        for ix in rows.indices where parent[ix] == nil && !placed[ix] {
            walk(ix, 0)
        }
        for ix in rows.indices where !placed[ix] {
            walk(ix, 0)
        }
        return order.map { ix, depth in
            var entry = rows[ix]
            let raw = rootDepth + depth
            let drawn = min(raw, Self.maxDepth)
            entry.depth = drawn
            if let lead = entry.lead, parent[ix] == nil || raw > drawn {
                entry.leadName = namesById[lead]
            }
            return entry
        }
    }
}

private extension RemoteAgent {
    var shown: AgentShown {
        AgentShown(
            agentId: agentId, chief: chief, name: name, cli: cli, title: title, titled: titled,
            session: session, group: group, turn: turn, subagents: subagents, tasks: tasks,
            failed: failed, nextTrigger: nextTrigger, line: line, calledAt: calledAt, face: face,
            lead: lead, project: project, projectName: projectName, reconnecting: reconnecting,
            snoozedUntil: snoozedUntil, settledAt: settledAt
        )
    }
}

/// The model and effort pickers a launcher's last session offered, as the core reads them.
struct LauncherConfig: Equatable {
    let model: ConfigPicker?
    let effort: ConfigPicker?

    init(_ launcher: LauncherInfo?) {
        model = launcher?.model
        effort = launcher?.effort
    }
}
