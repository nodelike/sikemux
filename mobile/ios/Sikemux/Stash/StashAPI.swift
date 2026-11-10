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
            try? await delete(item.id)
            throw error
        }
        return item
    }

    /// Stashes a file as its bytes, read from disk as it goes.
    static func upload(_ file: StashFile, prompt: String? = nil) async throws -> StashItem {
        var request = URLRequest(url: Config.apiUrl.appending(path: "v1/stash"))
        request.httpMethod = "POST"
        request.setValue("Bearer \(try await Account.shared.token())", forHTTPHeaderField: "authorization")
        request.setValue(file.mime, forHTTPHeaderField: "content-type")
        request.setValue(uriEncoded(file.name), forHTTPHeaderField: "x-stash-name")
        if let prompt { request.setValue(prompt, forHTTPHeaderField: "x-stash-prompt") }
        let (data, response) = try await offline { try await URLSession.shared.upload(for: request, fromFile: file.url) }
        try check(data, response)
        return try JSONDecoder().decode(StashItem.self, from: data)
    }

    /// Brings a stashed file down to this phone, once.
    static func download(_ item: StashItem) async throws -> URL {
        let folder = URL.cachesDirectory.appending(path: "stash/\(item.id)")
        let destination = folder.appending(path: item.name ?? "file")
        if FileManager.default.fileExists(atPath: destination.path) { return destination }
        var request = URLRequest(url: Config.apiUrl.appending(path: "v1/stash/\(item.id)/file"))
        request.setValue("Bearer \(try await Account.shared.token())", forHTTPHeaderField: "authorization")
        let (file, response) = try await offline { try await URLSession.shared.download(for: request) }
        guard (response as? HTTPURLResponse).map({ (200..<300).contains($0.statusCode) }) == true else {
            throw StashFailure("Couldn't get this from your stash.")
        }
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        try? FileManager.default.removeItem(at: destination)
        try FileManager.default.moveItem(at: file, to: destination)
        return destination
    }

    private static func offline<T>(_ send: () async throws -> T) async throws -> T {
        do { return try await send() } catch { throw StashFailure("Can't reach Sikemux. Check the phone is online.") }
    }

    private static func check(_ data: Data, _ response: URLResponse) throws {
        let status = (response as? HTTPURLResponse)?.statusCode ?? 0
        if (200..<300).contains(status) { return }
        let failure = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        throw StashFailure((failure?["error"] as? [String: Any])?["message"] as? String ?? "Sikemux answered \(status).")
    }

    private static func uriEncoded(_ text: String) -> String {
        let allowed = CharacterSet(charactersIn: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_.!~*'()")
        return text.addingPercentEncoding(withAllowedCharacters: allowed) ?? text
    }
}

struct StashFailure: LocalizedError {
    let message: String
    init(_ message: String) { self.message = message }
    var errorDescription: String? { message }
}
