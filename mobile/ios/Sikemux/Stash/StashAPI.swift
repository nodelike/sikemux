import Foundation

extension StashItem: Identifiable {
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
    static func list(cursor: String?) async throws -> StashList {
        var path = "/v1/stash?taken=false"
        if let cursor {
            path += "&cursor=\(cursor.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) ?? cursor)"
        }
        return try await AccountAPI.send("GET", path)
    }

    static func take(_ id: String) async throws {
        _ = try await AccountAPI.send("PATCH", "/v1/stash/\(id)", body: StashUpdate(taken: true), as: StashItem.self)
    }

    static func delete(_ id: String) async throws {
        try await AccountAPI.send("DELETE", "/v1/stash/\(id)")
    }

    static func seen(at: String) async throws -> String {
        try await AccountAPI.send("PUT", "/v1/stash/seen", body: StashSeen(at: at), as: StashSeen.self).at
    }

    /// Stashes a whole prompt as one item: its text, then each file under it. The server shows it only once every
    /// file is in; one that could not finish is taken back.
    static func stashPrompt(text: String, files: [StashFile]) async throws -> StashItem {
        let add = StashAdd.prompt(StashAddPrompt(text: text, attachmentCount: files.count))
        let item = try await AccountAPI.send("POST", "/v1/stash", body: add, as: StashItem.self)
        do {
            for file in files { _ = try await upload(file, prompt: item.id) }
        } catch {
            do { try await delete(item.id) } catch {
                print("sikemux: could not take back a half-stashed prompt: \(error)")
            }
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
