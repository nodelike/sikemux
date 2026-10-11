import Foundation

/// Any JSON value, kept whole for the parts the core passes through untouched.
enum JSONValue: Decodable, Equatable {
    case null
    case bool(Bool)
    case number(Double)
    case string(String)
    case array([JSONValue])
    case object([String: JSONValue])

    init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() {
            self = .null
        } else if let value = try? container.decode(Bool.self) {
            self = .bool(value)
        } else if let value = try? container.decode(Double.self) {
            self = .number(value)
        } else if let value = try? container.decode(String.self) {
            self = .string(value)
        } else if let value = try? container.decode([JSONValue].self) {
            self = .array(value)
        } else {
            self = .object(try container.decode([String: JSONValue].self))
        }
    }

    subscript(key: String) -> JSONValue? {
        if case .object(let fields) = self { return fields[key] }
        return nil
    }

    var string: String? {
        if case .string(let value) = self { return value }
        return nil
    }

    var array: [JSONValue] {
        if case .array(let values) = self { return values }
        return []
    }
}

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
    let configOptions: JSONValue?
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
}

struct Attention: Decodable, Equatable {
    let id: String
    let agentId: String
    let provider: String
    let cwd: String
    let request: JSONValue
    let at: Double
}

enum AgentState: Int, Comparable {
    case idle = 0
    case done = 1
    case working = 2
    case blocked = 3

    static func < (lhs: AgentState, rhs: AgentState) -> Bool { lhs.rawValue < rhs.rawValue }
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
    let subagents: Int
    /// Shells, monitors or subagents still going once its turn is over.
    let background: Bool
    /// A chat agent can be answered from the notch; a terminal agent only in Sikemux.
    let isChat: Bool
    let ask: Ask?
}

let agentNames: [String: String] = [
    "claude": "Claude",
    "codex": "Codex",
    "opencode": "OpenCode",
    "grok": "Grok",
    "pi": "Pi",
    "omp": "OMP",
    "hermes": "Hermes",
]

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

    /// Every agent open in the app, most in need of the person first, then
    /// the most recently active.
    var agents: [AgentItem] {
        var items: [(agent: AgentItem, activeAt: Double)] = []
        // A sleeping chat is still open in the app; it starts again when opened there.
        for chat in chats {
            let ask = attentions.first { $0.agentId == chat.agentId }.map(Self.ask)
            let state: AgentState =
                !chat.pendingPermissions.isEmpty ? .blocked : chat.running ? .working : chat.unread ? .done : .idle
            items.append((AgentItem(
                id: chat.agentId,
                provider: chat.provider,
                title: chat.title ?? "New \(agentName(chat.provider)) chat",
                project: projectName(chat.cwd),
                state: state,
                subagents: chat.subagents,
                background: chat.tasks > 0 || chat.subagents > 0,
                isChat: true,
                ask: ask
            ), chat.activeAt ?? 0))
        }
        for session in sessions where session.kind == "terminal" && session.running {
            guard let agentId = session.agentId, !items.contains(where: { $0.agent.id == agentId }) else { continue }
            let provider = session.agentType ?? "agent"
            let state: AgentState
            switch session.agentState {
            case "blocked": state = .blocked
            case "working": state = .working
            case "idle" where session.unread: state = .done
            default: state = .idle
            }
            items.append((AgentItem(
                id: agentId,
                provider: provider,
                title: session.title ?? agentName(provider),
                project: projectName(session.project),
                state: state,
                subagents: 0,
                background: false,
                isChat: false,
                ask: nil
            ), session.activeAt ?? 0))
        }
        return items.enumerated()
            .sorted { a, b in
                if a.element.agent.state != b.element.agent.state { return a.element.agent.state > b.element.agent.state }
                if a.element.activeAt != b.element.activeAt { return a.element.activeAt > b.element.activeAt }
                return a.offset < b.offset
            }
            .map(\.element.agent)
    }

    private static func ask(_ attention: Attention) -> Ask {
        let toolCall = attention.request["toolCall"]
        let raw = toolCall?["rawInput"]
        let shell = raw?["command"]?.string ?? raw?["cmd"]?.string
        let command = shell ?? toolCall?["title"]?.string ?? "a tool"
        func option(_ kind: String) -> String? {
            attention.request["options"]?.array.first { $0["kind"]?.string == kind }?["optionId"]?.string
        }
        return Ask(
            attentionId: attention.id,
            command: command,
            isCommand: shell != nil,
            allowOnce: option("allow_once"),
            allowAlways: option("allow_always"),
            reject: option("reject_once") ?? option("reject_always")
        )
    }
}

/// One choice a session offers, such as a model.
struct ConfigChoice: Equatable, Hashable {
    let value: String
    let label: String
}

struct ConfigSelect: Equatable {
    let id: String
    let current: String
    let choices: [ConfigChoice]
}

/// The model and effort pickers a launcher's last session offered, read the
/// way `src/chat/sessionConfig.ts` reads them.
struct LauncherConfig: Equatable {
    let model: ConfigSelect?
    let effort: ConfigSelect?

    private static let legacyEffort = ["claude": "effort", "codex": "reasoning_effort"]

    init(_ launcher: Launcher?) {
        let selects: [(category: String?, select: ConfigSelect)] = (launcher?.configOptions?.array ?? []).compactMap { row in
            guard row["type"]?.string == "select", let id = row["id"]?.string, let current = row["currentValue"]?.string
            else { return nil }
            return (row["category"]?.string, ConfigSelect(id: id, current: current, choices: Self.choices(row["options"])))
        }
        model = selects.first { $0.select.id == "model" }?.select
        let legacy = launcher.flatMap { Self.legacyEffort[$0.provider] }
        effort = (selects.first { $0.category == "thought_level" } ?? selects.first { $0.select.id == legacy })?.select
    }

    private static func choices(_ value: JSONValue?) -> [ConfigChoice] {
        (value?.array ?? []).flatMap { row -> [ConfigChoice] in
            if case .array = row["options"] ?? .null { return choices(row["options"]) }
            guard let value = row["value"]?.string, let name = row["name"]?.string else { return [] }
            return [ConfigChoice(value: value, label: versioned(name, row["description"]?.string))]
        }
    }

    /// The agent names a model without its release number ("Opus") and leaves
    /// that number in the description ("Opus 5 with 1M context"), so put it back.
    private static func versioned(_ label: String, _ description: String?) -> String {
        guard let description,
              let match = description.range(of: #"^\p{L}+\s+\d+(\.\d+)?\b"#, options: .regularExpression)
        else { return label }
        let named = description[match].split(separator: " ")
        guard named.count == 2, let head = label.split(separator: " ").first else { return label }
        let family = String(named[0])
        let version = String(named[1])
        guard head.lowercased() == family.lowercased(), !label.contains(version) else { return label }
        return label.replacingOccurrences(of: String(head), with: "\(head) \(version)", options: .anchored)
    }
}
