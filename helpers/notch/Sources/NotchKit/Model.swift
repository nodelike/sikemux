import Foundation

extension DeviceView {
    static let empty = DeviceView(
        workspace: Workspace(projects: [], launchers: []), sessions: [], chats: [], attentions: [],
        inbox: Inbox(items: [], working: 0, agents: [], rollup: nil),
        faceStyle: FaceStyle(motion: true)
    )
}

/// The marks beside a row and in the closed island's right wing.
enum Mark: Hashable {
    case needs, working, done, failed, idle
}

/// One group on the closed island's right wing.
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

/// A permission an agent waits on, with the person's choices.
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

/// One row of the island: an inbox item with how the core shows its agent.
struct Entry: Identifiable, Equatable {
    let item: InboxItem
    /// How the core shows the item's agent, while it is open here.
    let agent: AgentShown?
    let project: String
    let ask: Ask?
    var question: AgentQuestion? = nil
    /// The attempt its agent is retrying a failed request on, 0 when it did not say.
    var reconnecting: Int? = nil

    var id: String { item.id }
    var provider: String { item.cli ?? "" }
    var title: String { agent?.title ?? item.title ?? item.card?.title ?? item.what }
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
    var isChat: Bool { item.agentId != nil && item.session == nil && item.cli != nil }
    var since: Date? { item.at > 0 ? Date(timeIntervalSince1970: Double(item.at) / 1000) : nil }

    var mark: Mark {
        switch agent?.group {
        case .blocked: return .needs
        case .working: return .working
        case .done: return item.kind == .failed ? .failed : .done
        case .idle: return .idle
        case .unknown, nil: break
        }
        return switch item.tier {
        case .needs: .needs
        case .progress: .working
        case .review, .finished: item.kind == .failed ? .failed : .done
        case .unknown: .idle
        }
    }

    /// What the item says, and for a question why it reached the person rather than its lead.
    var what: String {
        [item.what, question?.viaNote].compactMap { $0 }.joined(separator: " · ")
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
    private func projectName(_ value: String?) -> String {
        guard let value, !value.isEmpty else { return "" }
        if let project = workspace.projects.first(where: { $0.id == value || $0.path == value }) {
            return project.name
        }
        return URL(fileURLWithPath: value).lastPathComponent
    }

    /// Every open agent once, in the order the core lists them on every device, each with what the inbox says about it
    /// and the question it asks from `questions`. Agents on other hosts the inbox lists follow; items that are not about
    /// an agent are left out.
    func entries(_ questions: [AgentQuestion]) -> [Entry] {
        let listed = Set(inbox.agents.map(\.agentId))
        var seen = Set<String>()
        let others = inbox.items.compactMap(\.agentId).filter { !listed.contains($0) && seen.insert($0).inserted }
        return (inbox.agents.map(\.agentId) + others).compactMap { agentId in
            let agent = inbox.agents.first { $0.agentId == agentId }
            guard let item = inbox.items.first(where: { $0.agentId == agentId }) ?? idleItem(agentId) else { return nil }
            return Entry(
                item: item,
                agent: agent,
                project: projectName(item.project),
                ask: attentions.first { $0.id == item.requestId }.map(Ask.init),
                question: questions.first { $0.id == item.questionId },
                reconnecting: chats.first { $0.agentId == agentId }?.reconnecting
            )
        }
    }

    private func idleItem(_ agentId: String) -> InboxItem? {
        if let chat = chats.first(where: { $0.agentId == agentId }) {
            return InboxItem(
                id: "agent:\(agentId)", tier: .unknown, kind: .unknown, agentId: agentId, name: chat.name, cli: chat.provider,
                project: chat.cwd, title: nil, what: "", detail: nil, at: 0,
                session: nil, card: nil, requestId: nil, questionId: nil, url: nil
            )
        }
        guard let session = sessions.first(where: { $0.agentId == agentId }) else { return nil }
        return InboxItem(
            id: "agent:\(agentId)", tier: .unknown, kind: .unknown, agentId: agentId, name: session.agentName,
            cli: session.agentType, project: session.project, title: nil, what: "", detail: nil, at: 0,
            session: session.id, card: nil, requestId: nil, questionId: nil, url: nil
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
