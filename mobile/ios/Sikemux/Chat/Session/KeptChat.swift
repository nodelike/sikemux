import Foundation

/// A chat as this phone last drew it, so opening it again draws at once and asks the host only for what came after.
struct KeptChat: Sendable {
    struct Kept: Codable, Sendable {
        var chat: ChatRows
        var feed: String
        var seq: UInt64
        /// Another build of the app may draw a chat in a shape this one does not read.
        var build = KeptChat.build
    }

    /// Past this many chats, the one written down longest ago is let go.
    private static let maxChats = 40
    /// A chat drawn bigger than this is opened from the host instead.
    private static let maxBytes = 2 * 1024 * 1024
    private static let build =
        (Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "")
        + "/" + (Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "")
    private static let folder = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
        .appendingPathComponent("chats")
    private static let writing = DispatchQueue(label: "sikemux.kept-chats", qos: .utility)

    let file: URL

    init(core: String, agentId: String) {
        let name = "\(core):\(agentId)".addingPercentEncoding(withAllowedCharacters: .alphanumerics) ?? agentId
        file = Self.folder.appendingPathComponent("\(name).json")
    }

    /// Off the main thread, as a kept chat can be megabytes of JSON.
    func load() async -> Kept? {
        let file = file
        let build = Self.build
        return await Task.detached(priority: .userInitiated) { () -> Kept? in
            guard FileManager.default.fileExists(atPath: file.path) else { return nil }
            do {
                let kept = try JSONDecoder().decode(Kept.self, from: Data(contentsOf: file))
                return kept.build == build ? kept : nil
            } catch {
                print("sikemux: could not read the chat kept on this phone: \(error)")
                return nil
            }
        }.value
    }

    /// Off the main thread, so leaving a chat never waits on writing it down.
    func save(_ kept: Kept) {
        let file = file
        Self.writing.async {
            do {
                let data = try JSONEncoder().encode(kept)
                guard data.count <= Self.maxBytes else { return }
                try FileManager.default.createDirectory(at: Self.folder, withIntermediateDirectories: true)
                try data.write(to: file, options: .atomic)
                Self.evictOldest()
            } catch {
                print("sikemux: could not keep the chat on this phone: \(error)")
            }
        }
    }

    private static func evictOldest() {
        let keys: Set<URLResourceKey> = [.contentModificationDateKey]
        guard
            let files = try? FileManager.default.contentsOfDirectory(
                at: folder, includingPropertiesForKeys: Array(keys)),
            files.count > maxChats
        else { return }
        let age = { (url: URL) in (try? url.resourceValues(forKeys: keys).contentModificationDate) ?? .distantPast }
        for url in files.sorted(by: { age($0) < age($1) }).prefix(files.count - maxChats) {
            do {
                try FileManager.default.removeItem(at: url)
            } catch {
                print("sikemux: could not let go of a kept chat: \(error)")
            }
        }
    }
}
