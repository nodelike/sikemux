import Foundation

struct DeviceView: Decodable, Equatable {
    let workspace: Workspace
    let chats: [ChatInfo]
    let sessions: [SessionInfo]
    let attentions: [Attention]
    let inbox: Inbox

    static let empty = DeviceView(workspace: Workspace(projects: [], launchers: []), chats: [], sessions: [], attentions: [], inbox: Inbox(items: [], working: 0))
}

struct Workspace: Decodable, Equatable {
    let projects: [Project]
    let launchers: [Launcher]
}

struct Project: Decodable, Equatable, Hashable {
    let id: String
    let name: String
    let path: String
}

struct Launcher: Decodable, Equatable {
    let id: String
    let provider: String
    let label: String
    let permissionMode: String
    let model: ConfigSelect?
    let effort: ConfigSelect?
}

/// An open chat, and what it still has going, which the inbox does not say.
struct ChatInfo: Decodable, Equatable {
    let agentId: String
    let name: String?
    let provider: String
    let title: String?
    let cwd: String
    /// Its place among the open agents, in the order the app's rail lists them: 0 first.
    let attentionRank: Int?
    let subagents: Int
    let tasks: Int
    /// The attempt the agent is retrying a failed request on, 0 when it did not say.
    let reconnecting: Int?
}

/// A terminal, which is an open agent when it carries one.
struct SessionInfo: Decodable, Equatable {
    let id: UInt64
    let running: Bool
    let project: String?
    let agentId: String?
    let agentName: String?
    let agentType: String?
    let title: String?
    let attentionRank: Int?
}

struct Attention: Decodable, Equatable {
    let id: String
    let agentId: String
    let choices: PermissionChoices
}

/// A permission request as the core reads it for the island's three buttons.
struct PermissionChoices: Decodable, Equatable {
    let allowOnce: String?
    let allowAlways: String?
    let reject: String?
    let command: String?
    let title: String?
}

/// What waits on the person and what is under way, in the order the core ranks it.
struct Inbox: Decodable, Equatable {
    let items: [InboxItem]
    /// Agents at work now, listed or not.
    let working: Int

    init(items: [InboxItem], working: Int) {
        self.items = items
        self.working = working
    }

    /// An item this build cannot read, such as one in a tier added later, is left out rather than losing the whole view.
    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        items = try container.decode([Readable].self, forKey: .items).compactMap(\.item)
        working = try container.decode(Int.self, forKey: .working)
    }

    private enum CodingKeys: String, CodingKey { case items, working }

    private struct Readable: Decodable {
        let item: InboxItem?
        init(from decoder: Decoder) throws { item = try? InboxItem(from: decoder) }
    }
}

struct InboxItem: Decodable, Equatable, Identifiable {
    enum Tier: String, Decodable {
        case needs, review, finished, progress
        /// An open agent the inbox has nothing on; the island lists it all the same.
        case idle
    }

    enum Kind: String, Decodable {
        case permission, question, input, held, assumption, pullRequest, changes, failed, finished, working
        /// One added after this build; its tier still places it.
        case other

        init(from decoder: Decoder) throws {
            self = Kind(rawValue: try decoder.singleValueContainer().decode(String.self)) ?? .other
        }
    }

    struct Card: Decodable, Equatable {
        let title: String
    }

    let id: String
    let tier: Tier
    let kind: Kind
    let agentId: String?
    /// What the core calls the agent, such as Otter.
    let name: String?
    let cli: String?
    let project: String?
    let title: String?
    /// What is needed or what happened, in the words the app and phones use.
    let what: String
    let detail: String?
    let at: Double
    /// The agent's terminal; none for a chat.
    let session: UInt64?
    let card: Card?
    let requestId: String?
    let questionId: String?
    let url: String?
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

/// A question an agent put to the person or its lead, as the core lists it.
struct AgentQuestion: Decodable, Equatable {
    let id: String
    let text: String
    let choices: [QuestionChoice]
    /// The lead it went to first, once it reached the person.
    let via: String?
    /// That lead passed it on itself rather than leaving it unanswered.
    let passed: Bool

    /// Why it reached the person rather than its lead.
    var why: String? {
        via.map { passed ? "\($0) didn't know" : "\($0) didn't answer" }
    }
}

struct QuestionChoice: Decodable, Equatable {
    let label: String
}

/// One row of the island: an inbox item with what the notch adds to it.
struct Entry: Identifiable, Equatable {
    let item: InboxItem
    let project: String
    /// Subagents, and shells or monitors, its agent still has going.
    let subagents: Int
    let tasks: Int
    let ask: Ask?
    var question: AgentQuestion? = nil
    /// The attempt its agent is retrying a failed request on, 0 when it did not say.
    var reconnecting: Int? = nil

    var id: String { item.id }
    var provider: String { item.cli ?? "" }
    var title: String { item.title ?? item.card?.title ?? item.what }
    var isChat: Bool { item.agentId != nil && item.session == nil && item.cli != nil }
    var since: Date? { item.at > 0 ? Date(timeIntervalSince1970: item.at / 1000) : nil }

    var mark: Mark {
        switch item.tier {
        case .needs: .needs
        case .progress: .working
        case .review, .finished: item.kind == .failed ? .failed : .done
        case .idle: .idle
        }
    }

    /// What the item says, and for a question why it reached the person rather than its lead.
    var what: String {
        [item.what, question?.why].compactMap { $0 }.joined(separator: " · ")
    }
}

func agentName(_ provider: String) -> String {
    agentNames[provider] ?? provider.capitalized
}

extension DeviceView {
    private func projectName(_ value: String?) -> String {
        guard let value, !value.isEmpty else { return "" }
        if let project = workspace.projects.first(where: { $0.id == value || $0.path == value }) {
            return project.name
        }
        return URL(fileURLWithPath: value).lastPathComponent
    }

    /// Every open agent once, in the app rail's order, each with what the inbox says about it and the question it asks
    /// from `questions`. Agents on other hosts the inbox lists follow; items that are not about an agent are left out.
    func entries(_ questions: [AgentQuestion]) -> [Entry] {
        let open = chats.map { (agentId: $0.agentId, rank: $0.attentionRank) }
            + sessions.compactMap { session in
                session.running ? session.agentId.map { (agentId: $0, rank: session.attentionRank) } : nil
            }
        let ranked = open.enumerated()
            .sorted { ($0.element.rank ?? Int.max, $0.offset) < ($1.element.rank ?? Int.max, $1.offset) }
            .map(\.element.agentId)
        let listed = Set(ranked)
        var seen = Set<String>()
        let others = inbox.items.compactMap(\.agentId).filter { !listed.contains($0) && seen.insert($0).inserted }
        return (ranked + others).compactMap { agentId in
            guard let item = inbox.items.first(where: { $0.agentId == agentId }) ?? idleItem(agentId) else { return nil }
            let chat = chats.first { $0.agentId == agentId }
            return Entry(
                item: item,
                project: projectName(item.project),
                subagents: chat?.subagents ?? 0,
                tasks: chat?.tasks ?? 0,
                ask: attentions.first { $0.id == item.requestId }.map(Ask.init),
                question: questions.first { $0.id == item.questionId },
                reconnecting: chat?.reconnecting
            )
        }
    }

    private func idleItem(_ agentId: String) -> InboxItem? {
        if let chat = chats.first(where: { $0.agentId == agentId }) {
            return InboxItem(
                id: "agent:\(agentId)", tier: .idle, kind: .other, agentId: agentId, name: chat.name, cli: chat.provider,
                project: chat.cwd, title: chat.title ?? "New \(agentName(chat.provider)) chat", what: "", detail: nil, at: 0,
                session: nil, card: nil, requestId: nil, questionId: nil, url: nil
            )
        }
        guard let session = sessions.first(where: { $0.agentId == agentId }) else { return nil }
        return InboxItem(
            id: "agent:\(agentId)", tier: .idle, kind: .other, agentId: agentId, name: session.agentName,
            cli: session.agentType, project: session.project, title: session.title, what: "", detail: nil, at: 0,
            session: session.id, card: nil, requestId: nil, questionId: nil, url: nil
        )
    }
}

/// One choice a session offers, such as a model.
struct ConfigChoice: Decodable, Equatable, Hashable {
    let value: String
    let label: String
}

struct ConfigSelect: Decodable, Equatable {
    let id: String
    let current: String
    let choices: [ConfigChoice]
}

/// The model and effort pickers a launcher's last session offered, as the core reads them.
struct LauncherConfig: Equatable {
    let model: ConfigSelect?
    let effort: ConfigSelect?

    init(_ launcher: Launcher?) {
        model = launcher?.model
        effort = launcher?.effort
    }
}
