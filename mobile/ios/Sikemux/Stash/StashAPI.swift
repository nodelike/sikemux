import Foundation

/// Something the person shared into Sikemux, kept on their account until they use it.
struct StashItem: Codable, Hashable, Identifiable {
    enum Kind: String, Codable {
        case link, text, image, video, file, prompt
    }

    struct Preview: Codable, Hashable {
        let title: String
        let site: String
        let description: String
        let imageUrl: String?
        let iconUrl: String?
    }

    let id: String
    let kind: Kind
    var url: String?
    var text: String?
    var attachments: [StashItem]?
    var note: String?
    var name: String?
    var mime: String?
    var size: Int?
    var preview: Preview?
    let createdAt: String
    var takenAt: String?

    var carriesFile: Bool { kind == .image || kind == .video || kind == .file }
}

/// A file on this phone to stash.
struct StashFile {
    let url: URL
    let name: String
    let mime: String
}

/// The account's stash on the accounts server.
enum StashAPI {
    struct Page: Decodable {
        let items: [StashItem]
        let cursor: String?
        let seenAt: String?
    }

    private struct Seen: Codable {
        let at: String
    }

    private struct Taken: Encodable {
        let taken = true
    }

    private struct PromptAdd: Encodable {
        let kind = "prompt"
        let text: String
        let attachmentCount: Int
    }

    static func list(cursor: String?) async throws -> Page {
        var path = "/v1/stash?taken=false"
        if let cursor { path += "&cursor=\(cursor.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) ?? cursor)" }
        return try await AccountAPI.send("GET", path)
    }

    static func take(_ id: String) async throws {
        let _: StashItem = try await AccountAPI.send("PATCH", "/v1/stash/\(id)", body: Taken())
    }

    static func delete(_ id: String) async throws {
        try await AccountAPI.send("DELETE", "/v1/stash/\(id)")
    }

    static func seen(at: String) async throws -> String {
        let seen: Seen = try await AccountAPI.send("PUT", "/v1/stash/seen", body: Seen(at: at))
        return seen.at
    }

    /// Stashes a whole prompt as one item: its text, then each file under it. The server shows it only once every
    /// file is in; one that could not finish is taken back.
    static func stashPrompt(text: String, files: [StashFile]) async throws -> StashItem {
        let item: StashItem = try await AccountAPI.send("POST", "/v1/stash", body: PromptAdd(text: text, attachmentCount: files.count))
        do {
            for file in files { _ = try await upload(file, prompt: item.id) }
        } catch {
            do { try await delete(item.id) } catch { print("sikemux: could not take back a half-stashed prompt: \(error)") }
            throw error
        }
        return item
    }

    /// Stashes a file as its bytes, read from disk as it goes.
    static func upload(_ file: StashFile, prompt: String? = nil) async throws -> StashItem {
        try await AccountAPI.upload(
            "/v1/stash", file: file.url, headers: StashRules.headers(mime: file.mime, name: file.name, prompt: prompt))
    }

    /// Brings a stashed file down to this phone, once.
    static func download(_ item: StashItem) async throws -> URL {
        let folder = URL.cachesDirectory.appending(path: "stash/\(item.id)")
        let destination = folder.appending(path: item.name ?? "file")
        if FileManager.default.fileExists(atPath: destination.path) { return destination }
        let file = try await AccountAPI.download("/v1/stash/\(item.id)/file")
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        try FileManager.default.moveItem(at: file, to: destination)
        return destination
    }
}
