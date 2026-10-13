import Foundation

struct DeviceView: Decodable, Equatable {
    let workspace: Workspace
    let sessions: [SessionInfo]
    let chats: [ChatInfo]
    let attentions: [Attention]

    static let empty = DeviceView(workspace: Workspace(projects: [], launchers: []), sessions: [], chats: [], attentions: [])
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

struct SessionInfo: Decodable, Equatable {
    let id: UInt64
    let kind: String
    let running: Bool
    let project: String?
    let agentId: String?
    let agentType: String?
    let agentState: String?
    let title: String?
    let unread: Bool
    let activeAt: Double?
    let group: AgentState?
    let status: AgentStatus?
    let attentionRank: Int?
}

struct ChatInfo: Decodable, Equatable {
    let agentId: String
    let provider: String
    let title: String?
    let cwd: String
    let state: String
    let running: Bool
    let pendingPermissions: [String]
    let subagents: Int
    let tasks: Int
    let permissionMode: String
    let asleep: Bool
    let unread: Bool
    let activeAt: Double?
    let group: AgentState
    let status: AgentStatus
    let attentionRank: Int
}

struct Attention: Decodable, Equatable {
    let id: String
    let agentId: String
    let provider: String
    let cwd: String
    let at: Double
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

/// The group the core puts an agent in, most in need of the person first.
enum AgentState: String, Decodable {
    case blocked, working, done, idle
}

/// What the core says an agent is doing: its group, or why it is not running.
enum AgentStatus: String, Decodable {
    case blocked, working, done, idle, asleep, stopped, starting

    /// The word for an agent that is not running, which its group does not say.
    var word: String? {
        switch self {
        case .asleep: "Sleeping"
        case .stopped: "Stopped"
        case .starting: "Starting"
        default: nil
        }
    }
}

/// One group on the closed island's right wing.
enum Rollup: Hashable {
    case state(AgentState)
    case stillRunning
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
}

/// One row of the island: an agent and what it needs from the person.
struct AgentItem: Identifiable, Equatable {
    let id: String
    let provider: String
    let title: String
    let project: String
    let state: AgentState
    let status: AgentStatus
    let subagents: Int
    /// Shells and monitors it still has going.
    let tasks: Int
    /// A chat agent can be answered from the notch; a terminal agent only in Sikemux.
    let isChat: Bool
    let ask: Ask?

    /// Subagents, shells or monitors still going once its turn is over.
    var background: Bool { subagents > 0 || tasks > 0 }
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

    /// Every agent open in the app, in the core's order: most in need of the person first.
    var agents: [AgentItem] {
        var items: [(agent: AgentItem, rank: Int)] = []
        // A sleeping chat is still open in the app; it starts again when opened there.
        for chat in chats {
            items.append((AgentItem(
                id: chat.agentId,
                provider: chat.provider,
                title: chat.title ?? "New \(agentName(chat.provider)) chat",
                project: projectName(chat.cwd),
                state: chat.group,
                status: chat.status,
                subagents: chat.subagents,
                tasks: chat.tasks,
                isChat: true,
                ask: attentions.first { $0.agentId == chat.agentId }.map(Self.ask)
            ), chat.attentionRank))
        }
        for session in sessions {
            guard let agentId = session.agentId, let rank = session.attentionRank,
                  let state = session.group, let status = session.status
            else { continue }
            let provider = session.agentType ?? "agent"
            items.append((AgentItem(
                id: agentId,
                provider: provider,
                title: session.title ?? agentName(provider),
                project: projectName(session.project),
                state: state,
                status: status,
                subagents: 0,
                tasks: 0,
                isChat: false,
                ask: nil
            ), rank))
        }
        return items.sorted { $0.rank < $1.rank }.map(\.agent)
    }

    private static func ask(_ attention: Attention) -> Ask {
        let choices = attention.choices
        return Ask(
            attentionId: attention.id,
            command: choices.command ?? choices.title ?? "a tool",
            isCommand: choices.command != nil,
            allowOnce: choices.allowOnce,
            allowAlways: choices.allowAlways,
            reject: choices.reject
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
