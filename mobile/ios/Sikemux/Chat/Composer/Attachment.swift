import Foundation

/// A photo or file picked on this phone for the next message.
struct Attachment: Codable, Hashable, Identifiable, Sendable {
    enum Kind: String, Codable, Sendable { case image, file }
    enum Upload: String, Codable, Sendable { case sending, failed }

    var id: String
    var kind: Kind
    var name: String
    var mime: String
    /// Where it is on this phone.
    var uri: URL
    var size: Int
    /// Where the host keeps it, once it is there.
    var path: String?
    var upload: Upload?
    var problem: String?
    /// The stash item it came from, which leaves the stash once the message is sent.
    var stashId: String?

    /// Only the host has the file, as for one the host put in the draft.
    var onHostOnly: Bool { path != nil && uri.path == path }

    private static let pictures: Set<String> = ["jpg", "jpeg", "png", "gif", "webp", "heic", "heif"]

    /// A file the host's draft holds, known here only by where the host keeps it.
    static func onHost(_ path: String) -> Attachment {
        let ext = (path as NSString).pathExtension.lowercased()
        return Attachment(
            id: "draft:\(path)", kind: pictures.contains(ext) ? .image : .file,
            name: (path as NSString).lastPathComponent, mime: "application/octet-stream",
            uri: URL(fileURLWithPath: path), size: 0, path: path
        )
    }
}

/// A message this phone wrote that the host has not taken yet.
struct Unsent: Hashable, Sendable {
    enum State: Sendable { case sending, failed }
    var state: State
    var problem: String?
}

enum Uploads {
    /// What of `picked` the host would take beside the `held` files already picked, and why the rest were left out.
    static func fitting(held: Int, _ picked: [Attachment]) -> (fits: [Attachment], notice: String?) {
        let sendable = { (attachment: Attachment) in
            attachment.path != nil || attachment.size <= WireLimits.maxAttachmentBytes
        }
        let tooBig = picked.filter { !sendable($0) }
        let small = picked.filter(sendable)
        let room = max(0, WireLimits.maxAttachments - held)
        let fits = Array(small.prefix(room))
        if !tooBig.isEmpty {
            let names = tooBig.map(\.name).joined(separator: ", ")
            let limit = WireLimits.maxAttachmentBytes / (1024 * 1024)
            return (fits, "\(names) \(tooBig.count == 1 ? "is" : "are") over \(limit) MB, too big to send")
        }
        return (fits, small.count > room ? "a message takes at most \(WireLimits.maxAttachments) files" : nil)
    }

    /// Where the host keeps each file that got there.
    static func paths(_ attachments: [Attachment]) -> [String] {
        attachments.compactMap(\.path)
    }

    /// Puts each file the host does not have yet on it, one at a time, marking each as it goes. Answers them all
    /// once every one got there, or nil when one did not.
    @MainActor
    static func each(
        _ attachments: [Attachment],
        put: (Attachment) async throws -> String,
        mark: (String, (inout Attachment) -> Void) -> Void
    ) async -> (sent: [Attachment]?, problem: String?) {
        let waiting = attachments.filter { $0.path == nil }
        for attachment in waiting {
            mark(attachment.id) {
                $0.upload = .sending
                $0.problem = nil
            }
        }
        var sent = attachments
        var failure: String?
        for attachment in waiting {
            do {
                let path = try await put(attachment)
                if let at = sent.firstIndex(where: { $0.id == attachment.id }) { sent[at].path = path }
                mark(attachment.id) {
                    $0.upload = nil
                    $0.path = path
                }
            } catch {
                let why = problem(error)
                failure = failure ?? why
                mark(attachment.id) {
                    $0.upload = .failed
                    $0.problem = why
                }
            }
        }
        return failure == nil ? (sent, nil) : (nil, failure)
    }

    static func sizeLabel(_ bytes: Int) -> String {
        if bytes < 1024 { return "\(bytes) B" }
        if bytes < 1024 * 1024 { return "\(Int((Double(bytes) / 1024).rounded())) KB" }
        return String(format: "%.1f MB", Double(bytes) / (1024 * 1024))
    }
}
