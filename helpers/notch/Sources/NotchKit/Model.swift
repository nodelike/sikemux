import Foundation

extension DeviceView {
    static let empty = DeviceView(
        workspace: Workspace(projects: [], launchers: []), attentions: [],
        inbox: Inbox(items: [], working: 0, agents: [], rollup: nil), calls: [],
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
    /// build does not know.
    init?(turn: TurnState) {
        switch turn.state {
        case .working: self = .working
        case .waiting: self = turn.wait.map(Mark.fills) == true ? .waiting : .timer
        case .needsYou: self = .needs
        case .done: self = .done
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
    case shells

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

    var id: String { item?.id ?? "agent:\(agent?.agentId ?? "")" }
    var agentId: String? { agent?.agentId ?? item?.agentId }
    var name: String? { item?.name ?? agent?.name }
    var provider: String { item?.cli ?? agent?.cli ?? "" }
    var project: String { item?.projectName ?? agent?.projectName ?? "" }
    /// What the core says its row leads with; an agent the inbox has nothing about goes by its title.
    var title: String { item?.heading ?? agent?.title ?? "" }
    /// The attempt its agent is retrying a failed request on, 0 when it did not say.
    var reconnecting: Int? { agent?.reconnecting }
    /// Subagents, and shells or monitors, its agent still has going.
    var subagents: Int { agent?.subagents ?? 0 }
    var tasks: Int { agent?.tasks ?? 0 }
    /// When its agent was last named with `@` or messaged by another agent.
    var calledAt: Int? { agent?.calledAt }
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
        let agents = inbox.agents.filter { $0.snoozedUntil == nil && $0.settledAt == nil }
        let hidden = Set(inbox.agents.map(\.agentId)).subtracting(agents.map(\.agentId))
        let listed = Set(agents.map(\.agentId))
        var seen = Set<String>()
        let others = inbox.items.compactMap(\.agentId)
            .filter { !listed.contains($0) && !hidden.contains($0) && seen.insert($0).inserted }
        return (agents.map(\.agentId) + others).map { agentId in
            let item = inbox.items.first { $0.agentId == agentId }
            return Entry(
                item: item,
                agent: agents.first { $0.agentId == agentId },
                ask: attentions.first { $0.id == item?.requestId }.map(Ask.init),
                question: questions.first { $0.id == item?.questionId }
            )
        }
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
