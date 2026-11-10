import Foundation

extension DeviceView {
    static let empty = DeviceView(
        workspace: Workspace(projects: [], launchers: []), attentions: [],
        inbox: Inbox(items: [], working: 0, agents: [], rollup: nil),
        faceStyle: FaceStyle(motion: true)
    )
}

enum Mark: Hashable {
    case needs, working, done, failed, idle
}

enum Rollup: Hashable {
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

enum HoldAnswer {
    case refuse, once, always
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
    /// When a trigger next wakes its agent, shown only while the agent waits.
    var nextTrigger: Date? {
        guard mark == .idle || mark == .done, let at = agent?.nextTrigger else { return nil }
        return Date(timeIntervalSince1970: Double(at) / 1000)
    }
    /// When its agent was last named with `@` or messaged by another agent.
    var calledAt: Int? { agent?.calledAt }
    var isChat: Bool { agentId != nil && session == nil && !provider.isEmpty }
    var session: Int? { agent?.session ?? item?.session }
    var since: Date? { item.map { Date(timeIntervalSince1970: Double($0.at) / 1000) } }

    var mark: Mark {
        switch agent?.group {
        case .blocked: return .needs
        case .working: return .working
        case .done: return agent?.failed == true ? .failed : .done
        case .idle: return .idle
        case .unknown, nil: break
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
    /// Every open agent once, in the order the core lists them on every device, each with what the inbox says about it
    /// and the question it asks from `questions`. Agents on other hosts the inbox lists follow; items that are not about
    /// an agent are left out.
    func entries(_ questions: [AgentQuestion]) -> [Entry] {
        let listed = Set(inbox.agents.map(\.agentId))
        var seen = Set<String>()
        let others = inbox.items.compactMap(\.agentId).filter { !listed.contains($0) && seen.insert($0).inserted }
        return (inbox.agents.map(\.agentId) + others).map { agentId in
            let item = inbox.items.first { $0.agentId == agentId }
            return Entry(
                item: item,
                agent: inbox.agents.first { $0.agentId == agentId },
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
