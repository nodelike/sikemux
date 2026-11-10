import Foundation

/// Files a chat's messages name on the host, copied to this phone once each.
@MainActor
final class HostFiles {
    static let shared = HostFiles()

    /// A file that just failed is asked for again only once this has passed.
    private static let retryAfter: TimeInterval = 60
    private static let location = #"(?::\d+){1,2}$|#L\d+(?:C\d+)?$"#
    private static let drawn: Set<String> = ["png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "avif", "tif", "tiff", "heic", "heif"]

    private var fetching: [String: Task<URL?, Never>] = [:]
    private var failedAt: [String: Date] = [:]

    /// A picture the phone can fetch from the host: a whole path, since it does not know which folder a chat works
    /// in. An SVG is left out, as the phone has nothing to draw it with.
    nonisolated static func imagePath(_ raw: String) -> String? {
        var path = raw.replacingOccurrences(of: location, with: "", options: .regularExpression)
        if path.hasPrefix("file://") {
            guard let url = URL(string: path) else { return nil }
            path = url.path
        }
        guard path.hasPrefix("/") else { return nil }
        return drawn.contains((path as NSString).pathExtension.lowercased()) ? path : nil
    }

    /// Whether the phone draws `raw` as a file: a folder and an extension, or a path from the root at least two
    /// folders deep.
    nonisolated static func namesFile(_ raw: String) -> Bool {
        let body = raw.replacingOccurrences(of: location, with: "", options: .regularExpression)
        guard body.contains("/") else { return false }
        return body.range(of: #"\.[A-Za-z0-9]{1,8}$"#, options: .regularExpression) != nil
            || body.range(of: #"^~?/[^/\s]+/[^/\s]"#, options: .regularExpression) != nil
    }

    private func saved(_ core: String, _ path: String) -> URL {
        FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("chat-files")
            .appendingPathComponent(String(core.prefix(16)))
            .appendingPathComponent((path as NSString).lastPathComponent)
    }

    /// Where the host's file `path` is on this phone, fetched first when it is not here yet.
    func read(core: String, path: String) async -> URL? {
        let file = saved(core, path)
        if FileManager.default.fileExists(atPath: file.path) { return file }
        let key = "\(core)/\(path)"
        if let failed = failedAt[key], Date().timeIntervalSince(failed) < Self.retryAfter { return nil }
        if let running = fetching[key] { return await running.value }
        guard let connection = Hub.shared.live(core).connection else { return nil }
        let task = Task { () -> URL? in
            do {
                try FileManager.default.createDirectory(at: file.deletingLastPathComponent(), withIntermediateDirectories: true)
                let params = String(decoding: try JSONEncoder().encode(["path": path]), as: UTF8.self)
                // The Rust client decodes and writes the file, so its bytes never pass through Swift.
                _ = try await connection.callToFile(method: "chatFile", paramsJson: params, field: "data", path: file.path)
                return file
            } catch {
                return nil
            }
        }
        fetching[key] = task
        let found = await task.value
        fetching[key] = nil
        if found == nil { failedAt[key] = Date() } else { failedAt[key] = nil }
        return found
    }
}
