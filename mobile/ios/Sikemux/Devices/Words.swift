import Foundation

/// The host's words for its projects and what runs in them, as every device screen names them.
extension DeviceView {
    func project(at place: String) -> ProjectInfo? {
        workspace.projects.first { $0.id == place || $0.path == place }
    }

    /// The name of the project with this id or folder, else the last folder of `path`.
    func projectName(_ place: String, path: String? = nil) -> String {
        project(at: place)?.name ?? folderName(path ?? place)
    }

    /// The id of the project the host says the agent is in; none for one outside every project.
    func projectOf(agent agentId: String) -> String? {
        inbox.agents.first { $0.agentId == agentId }?.project
    }

    /// How many things in `project` wait on the person, as the host counts them.
    func needs(in project: ProjectInfo) -> Int {
        inbox.needs[project.id] ?? 0
    }
}

func folderName(_ path: String) -> String {
    path.split(separator: "/").last.map(String.init) ?? path
}

/// Whether something on the host is in the project and for the agent the screen shows.
func listed(_ scope: ProjectInfo?, _ provider: String, project: String?, type: String?) -> Bool {
    (scope == nil || project == scope?.id) && (provider == "all" || type == provider)
}

/// The agent list's groups folded shut, by key, as the Mac's rail keeps its own.
@MainActor @Observable
final class Folded {
    static let shared = Folded()
    private static let key = "sikemux.folded-agent-groups"

    private(set) var shut: Set<String> = Set(UserDefaults.standard.stringArray(forKey: key) ?? ["settled"])

    func toggle(_ group: String) {
        if shut.contains(group) { shut.remove(group) } else { shut.insert(group) }
        UserDefaults.standard.set(Array(shut), forKey: Self.key)
    }
}

extension PairedDevice.Kind {
    var icon: IconDef {
        switch self {
        case .laptop: Icons.laptop
        case .desktop: Icons.desktop
        case .mini: Icons.mini
        }
    }
}
