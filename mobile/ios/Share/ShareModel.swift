import LinkPresentation
import SwiftUI
import UniformTypeIdentifiers

/// A chat the app wrote down for the extension, as its share sheet lists it.
struct ShareChat: Decodable, Identifiable {
    let host: String
    let hostName: String
    let chatId: String
    let provider: String
    let title: String
    let name: String?
    let project: String
    var id: String { "\(host):\(chatId)" }
}

struct ShareNewChat: Decodable {
    let host: String
    let hostName: String
    let project: String
    let projectPath: String
}

private struct Snapshot: Decodable {
    let apiUrl: String
    let chats: [ShareChat]
    let newChat: ShareNewChat?
}

enum Outgoing {
    case link(URL)
    case text(String)
    case file(url: URL, name: String, mime: String, size: Int64)
}

private let thumbnailPixels: CGFloat = 240
private let picturesShown = 4

@MainActor
final class ShareModel: ObservableObject {
    enum Phase {
        case loading
        case ready
        case sending(Double)
        case done(title: String, detail: String)
        case failed(String)
        case signedOut
    }

    @Published var phase: Phase = .loading
    @Published var note = ""
    @Published var items: [Outgoing] = []
    @Published var title: String?
    @Published var picture: UIImage?
    @Published var thumbnails: [UIImage] = []
    /// Called once the confirmation has shown, to close the sheet.
    var onFinish: (() -> Void)?
    private(set) var chats: [ShareChat] = []
    private(set) var newChat: ShareNewChat?
    private var upload: StashUpload?
    private var lastSend: (() -> Void)?
    private let folder = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)

    var tooBig: [String] {
        items.compactMap { item in
            if case .file(_, let name, _, let size) = item, size > StashRules.mostBytes { return name }
            return nil
        }
    }

    var sendable: [Outgoing] {
        items.filter { item in
            if case .file(_, _, _, let size) = item { return size <= StashRules.mostBytes }
            return true
        }
    }

    func load(_ inputs: [NSExtensionItem]) async {
        guard let snapshot = Self.snapshot(), let key = StashShared.key(),
            let upload = StashUpload(api: snapshot.apiUrl, keyHex: key)
        else {
            phase = .signedOut
            return
        }
        self.upload = upload
        chats = snapshot.chats
        newChat = snapshot.newChat
        do {
            try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        } catch {
            phase = .failed(error.localizedDescription)
            return
        }
        var found: [Outgoing] = []
        for input in inputs {
            for provider in input.attachments ?? [] {
                if let item = await read(provider) { found.append(item) }
            }
            if title == nil,
                let words = input.attributedContentText?.string.trimmingCharacters(in: .whitespacesAndNewlines),
                !words.isEmpty
            {
                title = words
            }
        }
        items = deduplicated(found)
        phase = .ready
        if case .link(let url)? = items.first { await describe(url) }
    }

    func send(to target: StashTarget?, done: (title: String, detail: String)) {
        lastSend = { [weak self] in self?.send(to: target, done: done) }
        guard case .ready = phase else { return }
        start(target: target, done: done)
    }

    func retry() {
        phase = .ready
        lastSend?()
    }

    func toChat(_ chat: ShareChat) {
        send(
            to: StashTarget(hostKey: chat.host, chatId: chat.chatId),
            done: ("Added to \(chat.name.map { "@\($0)" } ?? "the chat")'s draft", "Unsent, on \(chat.hostName)"))
    }

    func toNewChat(_ chat: ShareNewChat) {
        send(
            to: StashTarget(hostKey: chat.host, projectPath: chat.projectPath, newChat: true),
            done: ("In a new chat in \(chat.project)", "Unsent, on \(chat.hostName)"))
    }

    func toStash() {
        send(to: nil, done: ("Stashed", "Attach it from + in any chat."))
    }

    func cleanUp() {
        do {
            try FileManager.default.removeItem(at: folder)
        } catch {
            print("sikemux: could not clear what the share copied: \(error)")
        }
    }

    /// What the app left; nil until it has signed in, which sends the person there first.
    private static func snapshot() -> Snapshot? {
        guard let data = StashShared.read() else { return nil }
        do {
            return try JSONDecoder().decode(Snapshot.self, from: data)
        } catch {
            print("sikemux: could not read the chats the app left: \(error)")
            return nil
        }
    }

    private func start(target: StashTarget?, done: (title: String, detail: String)) {
        guard let upload else { return }
        let items = sendable
        guard !items.isEmpty else { return }
        let note = note.trimmingCharacters(in: .whitespacesAndNewlines)
        let noted = note.isEmpty ? nil : note
        phase = .sending(0)
        let weights = items.map { item -> Double in
            if case .file(_, _, _, let size) = item { return Double(max(size, 1)) }
            return 1
        }
        let total = weights.reduce(0, +)
        Task {
            do {
                var sent = 0.0
                for (index, item) in items.enumerated() {
                    let before = sent
                    let weight = weights[index]
                    switch item {
                    case .link(let url):
                        try await upload.link(url.absoluteString, note: noted, target: target)
                    case .text(let text) where text.count <= StashRules.mostText:
                        try await upload.text(text, note: noted, target: target)
                    case .text(let text):
                        let file = folder.appendingPathComponent("Shared text.txt")
                        try Data(text.utf8).write(to: file)
                        try await upload.file(
                            at: file, name: "Shared text.txt", mime: "text/plain", note: noted, target: target
                        ) { _ in }
                    case .file(let url, let name, let mime, _):
                        try await upload.file(at: url, name: name, mime: mime, note: noted, target: target) { share in
                            self.phase = .sending((before + share * weight) / total)
                        }
                    }
                    sent += weight
                    phase = .sending(sent / total)
                }
                phase = .done(title: done.title, detail: done.detail)
                UINotificationFeedbackGenerator().notificationOccurred(.success)
                try? await Task.sleep(nanoseconds: 900_000_000)
                onFinish?()
            } catch StashProblem.signedOut {
                phase = .signedOut
            } catch {
                UINotificationFeedbackGenerator().notificationOccurred(.error)
                phase = .failed(error.localizedDescription)
            }
        }
    }

    /// A file a share arrives with is readable only while it is being handed over, so it is copied.
    private func read(_ provider: NSItemProvider) async -> Outgoing? {
        if provider.hasItemConformingToTypeIdentifier(UTType.url.identifier),
            !provider.hasItemConformingToTypeIdentifier(UTType.fileURL.identifier),
            let url = try? await provider.loadItem(forTypeIdentifier: UTType.url.identifier) as? URL, !url.isFileURL
        {
            return .link(url)
        }
        if provider.hasItemConformingToTypeIdentifier(UTType.plainText.identifier),
            !provider.hasItemConformingToTypeIdentifier(UTType.fileURL.identifier),
            let text = try? await provider.loadItem(forTypeIdentifier: UTType.plainText.identifier) as? String
        {
            let words = text.trimmingCharacters(in: .whitespacesAndNewlines)
            if let url = URL(string: words), url.scheme == "http" || url.scheme == "https" { return .link(url) }
            return words.isEmpty ? nil : .text(words)
        }
        for type in [UTType.image, .movie, .pdf, .data, .item]
        where provider.hasItemConformingToTypeIdentifier(type.identifier) {
            if let file = await copy(provider, type: type) { return file }
        }
        return nil
    }

    private func copy(_ provider: NSItemProvider, type: UTType) async -> Outgoing? {
        let folder = self.folder
        let suggested = provider.suggestedName
        let copied: (URL, String, String, Int64)? = await withCheckedContinuation { done in
            provider.loadFileRepresentation(forTypeIdentifier: type.identifier) { url, _ in
                guard let url else { return done.resume(returning: nil) }
                let kind = UTType(filenameExtension: url.pathExtension) ?? type
                var name = url.lastPathComponent
                if let suggested, !suggested.isEmpty {
                    name = url.pathExtension.isEmpty ? suggested : "\(suggested).\(url.pathExtension)"
                }
                let target = folder.appendingPathComponent(UUID().uuidString).appendingPathComponent(name)
                do {
                    try FileManager.default.createDirectory(
                        at: target.deletingLastPathComponent(), withIntermediateDirectories: true)
                    try FileManager.default.copyItem(at: url, to: target)
                    let size = (try? target.resourceValues(forKeys: [.fileSizeKey]).fileSize).map(Int64.init) ?? 0
                    done.resume(returning: (target, name, kind.preferredMIMEType ?? "application/octet-stream", size))
                } catch {
                    done.resume(returning: nil)
                }
            }
        }
        guard let (url, name, mime, size) = copied else { return nil }
        if mime.hasPrefix("image/"), thumbnails.count < picturesShown, let small = Self.thumbnail(url) {
            thumbnails.append(small)
        }
        return .file(url: url, name: name, mime: mime, size: size)
    }

    /// A link shared with its own text as well comes once.
    private func deduplicated(_ found: [Outgoing]) -> [Outgoing] {
        let links = found.compactMap { item -> String? in
            if case .link(let url) = item { return url.absoluteString }
            return nil
        }
        var seen = Set<String>()
        return found.filter { item in
            switch item {
            case .link(let url): return seen.insert(url.absoluteString).inserted
            case .text(let text): return !links.contains(text)
            case .file: return true
            }
        }
    }

    /// The page's own title and picture, when iOS can read them in a moment.
    private func describe(_ url: URL) async {
        let provider = LPMetadataProvider()
        provider.timeout = 4
        guard let metadata = try? await provider.startFetchingMetadata(for: url) else { return }
        if let found = metadata.title, !found.isEmpty { title = found }
        guard let images = metadata.imageProvider else { return }
        let image: UIImage? = await withCheckedContinuation { done in
            images.loadObject(ofClass: UIImage.self) { object, _ in done.resume(returning: object as? UIImage) }
        }
        if let image { picture = Self.scaled(image, width: 720) }
    }

    private static func thumbnail(_ url: URL) -> UIImage? {
        guard let source = CGImageSourceCreateWithURL(url as CFURL, nil) else { return nil }
        let options: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceThumbnailMaxPixelSize: thumbnailPixels,
        ]
        return CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary).map { UIImage(cgImage: $0) }
    }

    private static func scaled(_ image: UIImage, width: CGFloat) -> UIImage {
        guard image.size.width > width else { return image }
        let size = CGSize(width: width, height: image.size.height * width / image.size.width)
        return UIGraphicsImageRenderer(size: size).image { _ in image.draw(in: CGRect(origin: .zero, size: size)) }
    }
}
