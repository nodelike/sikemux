import Foundation

/// Keeps what the share extension needs where it can read it while signed in: the chats used last, where the
/// accounts server is, and this phone's key. Signing out takes them all away.
@MainActor
enum ShareExtension {
    private struct Chat: Encodable, Equatable {
        let host: String
        let hostName: String
        let chatId: String
        let provider: String
        let title: String
        let name: String?
        let project: String
    }

    private struct NewChat: Encodable, Equatable {
        let host: String
        let hostName: String
        let project: String
        let projectPath: String
    }

    private struct Snapshot: Encodable, Equatable {
        let apiUrl: String
        let chats: [Chat]
        let newChat: NewChat?
    }

    private static let shown = 4
    /// Hosts change their chats in bursts; the extension's copy follows once they settle.
    private static let settle: Duration = .seconds(1)

    private static var written: Snapshot?
    private static var settling: Task<Void, Never>?
    private static var signedIn = false

    static func signIn() {
        signedIn = true
        do { StashShared.setKey(try Identity.shared.keyHex()) } catch {
            print("sikemux: could not hand the share extension the key of this phone: \(error)")
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

    /// A host sent a new view, which may change its chats.
    static func changed() {
        guard signedIn else { return }
        settling?.cancel()
        settling = Task {
            try? await Task.sleep(for: settle)
            if !Task.isCancelled { write() }
        }
    }

    /// The chats used last across every host this phone has full access to, and a new chat in the newest one's project.
    private static func write() {
        var found: [(chat: Chat, path: String, at: Int)] = []
        for device in Hub.shared.devices where device.access == "full" {
            guard let view = Hub.shared.live(device.core).view else { continue }
            for chat in view.chats {
                let shown = view.inbox.agents.first { $0.agentId == chat.agentId }
                let place = shown?.project ?? chat.cwd
                let project = view.workspace.projects.first { $0.id == place || $0.path == place }
                found.append(
                    (
                        Chat(
                            host: device.core, hostName: device.displayName, chatId: chat.agentId,
                            provider: chat.provider,
                            title: chat.title ?? shown?.title ?? "New chat", name: chat.name,
                            project: project?.name ?? URL(fileURLWithPath: chat.cwd).lastPathComponent),
                        project?.path ?? chat.cwd,
                        shown?.activeAt ?? 0
                    ))
            }
        }
        found.sort { $0.at > $1.at }
        // Hosts not reached yet since launch know no chats; the last ones written stay until one is.
        if found.isEmpty, written != nil { return }
        let newChat = found.first.map {
            NewChat(host: $0.chat.host, hostName: $0.chat.hostName, project: $0.chat.project, projectPath: $0.path)
        }
        let next = Snapshot(
            apiUrl: Config.apiUrl.absoluteString, chats: found.prefix(shown).map(\.chat), newChat: newChat)
        guard next != written else { return }
        do {
            StashShared.write(try JSONEncoder().encode(next))
            written = next
        } catch {
            print("sikemux: could not write the share extension its chats: \(error)")
        }
    }
}
