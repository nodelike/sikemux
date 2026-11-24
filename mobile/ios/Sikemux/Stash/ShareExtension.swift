import Foundation
import OSLog

private let log = Logger(subsystem: "com.nodelike.sikemux", category: "stash")

/// Keeps what the share extension needs where it can read it while signed in: every project's chief,
/// where the accounts server is, and this phone's key. Signing out takes them all away.
@MainActor
enum ShareExtension {
    /// Hosts change their views in bursts; the extension's copy follows once they settle.
    private static let settle: Duration = .seconds(1)
    private static let lastProjectKey = "sikemux.last-open-project"

    private static var written: ShareSnapshot?
    private static var settling: Task<Void, Never>?
    private static var signedIn = false
    private static var lastOpen: (core: String, project: String)? = loadLastOpen()

    private static func loadLastOpen() -> (core: String, project: String)? {
        guard let stored = UserDefaults.standard.dictionary(forKey: lastProjectKey) as? [String: String],
            let core = stored["core"], let project = stored["project"]
        else { return nil }
        return (core, project)
    }

    /// Remembers the project the phone had open last, so its chief leads the share sheet.
    static func noteOpenProject(core: String, project: String?) {
        guard let project else { return }
        if lastOpen?.core == core && lastOpen?.project == project { return }
        lastOpen = (core, project)
        UserDefaults.standard.set(["core": core, "project": project], forKey: lastProjectKey)
        changed()
    }

    static func signIn() {
        signedIn = true
        do { StashShared.setKey(try Identity.shared.keyHex()) } catch {
            log.error("could not hand the share extension the key of this phone: \(error, privacy: .public)")
        }
        write()
    }

    static func signOut() {
        signedIn = false
        settling?.cancel()
        written = nil
        StashShared.write(nil)
        StashShared.setKey(nil)
    }

    /// A host sent a new view, which may change its project chiefs.
    static func changed() {
        guard signedIn else { return }
        settling?.cancel()
        settling = Task {
            try? await Task.sleep(for: settle)
            if !Task.isCancelled { write() }
        }
    }

    private static func lastOpenProject() -> (core: String, project: String)? {
        if let lastOpen { return lastOpen }
        let scoped = Hub.shared.devices
            .filter { $0.project != nil }
            .max { ($0.lastSeen ?? 0) < ($1.lastSeen ?? 0) }
        guard let scoped, let project = scoped.project else { return nil }
        return (scoped.core, project)
    }

    /// One row per project that has a chief across every host this phone has full access to, ordered with
    /// the project the phone last had open first, then most recently active.
    private static func write() {
        var found: [(chief: ShareSnapshot.Chief, at: Int, order: Int)] = []
        var order = 0
        for device in Hub.shared.devices where device.access == .full {
            guard let view = Hub.shared.live(device.core).view else { continue }
            var seenProjects = Set<String>()
            for project in view.workspace.projects {
                guard let chatId = project.chief, project.chiefHome == nil else { continue }
                seenProjects.insert(project.id)
                let chat = view.chats.first { $0.agentId == chatId }
                let shown =
                    view.inbox.agents.first { $0.agentId == chatId }
                    ?? view.inbox.kept?.first { $0.agentId == chatId }
                let recent = view.recent.first {
                    $0.agentId == chatId || ($0.project == project.id && $0.title == "Chief of staff")
                }
                let provider =
                    chat?.provider ?? shown?.cli ?? project.launcher?.split(separator: ":").first.map(String.init)
                    ?? "agent"
                let at = shown?.activeAt ?? chat?.activeAt ?? recent?.activeAt ?? 0
                found.append(
                    (
                        ShareSnapshot.Chief(
                            host: device.core, hostName: device.displayName, projectId: project.id,
                            project: project.name, chatId: chatId, provider: provider),
                        at,
                        order
                    ))
                order += 1
            }
            // Older cores without project.chief still mark open chiefs in inbox.agents.
            for shown in view.inbox.agents where shown.chief == true {
                guard let projectId = shown.project, !seenProjects.contains(projectId) else { continue }
                seenProjects.insert(projectId)
                let projectName = view.project(id: projectId)?.name ?? shown.projectName ?? projectId
                found.append(
                    (
                        ShareSnapshot.Chief(
                            host: device.core, hostName: device.displayName, projectId: projectId,
                            project: projectName, chatId: shown.agentId, provider: shown.cli),
                        shown.activeAt ?? 0,
                        order
                    ))
                order += 1
            }
        }
        let last = lastOpenProject()
        let isLast = { (chief: ShareSnapshot.Chief) in
            last != nil && chief.host == last?.core && chief.projectId == last?.project
        }
        found.sort { a, b in
            let aLast = isLast(a.chief)
            let bLast = isLast(b.chief)
            if aLast != bLast { return aLast }
            if a.at != b.at { return a.at > b.at }
            return a.order < b.order
        }
        // Hosts not reached yet since launch know no chiefs; the last ones written stay until one is.
        if found.isEmpty, written != nil { return }
        let next = ShareSnapshot(apiUrl: Config.apiUrl.absoluteString, chiefs: found.map(\.chief))
        guard next != written else { return }
        StashShared.write(next)
        written = next
    }
}
