import Foundation
import OSLog
import Observation
import SikemuxCore

private let log = Logger(subsystem: "com.nodelike.sikemux", category: "draft")

/// What the composer holds for the chat's next message, saved as the chat's own draft on the host.
@MainActor @Observable
final class ComposerDraft {
    private(set) var text = ""
    /// Picked for the next message.
    var attachments: [Attachment] = []
    /// Links and text the draft carries, as the host keeps them: shown as chips, sent as words.
    var chips: [ChatContext] = []
    /// The agents, files, chats and issues the draft names, over the words that name them.
    private(set) var tokens: [TextToken] = []
    /// The issues and pull requests the draft's chips name, by chip.
    @ObservationIgnored private var tracked: [String: ChatContext] = [:]
    /// The host's revision of this chat's draft that the composer last took in.
    @ObservationIgnored private var rev: Int?
    /// Changes made here since the draft was last saved, which the host's draft must not replace.
    @ObservationIgnored private var edits = 0
    @ObservationIgnored private var saving: Task<Void, Never>?

    @ObservationIgnored private let agentId: String
    /// Where the unsaved draft is kept on this phone.
    @ObservationIgnored private let key: String
    @ObservationIgnored private let held: () -> ChatDraft?
    @ObservationIgnored private let live: () -> Connection?
    @ObservationIgnored private let emit: () -> Void

    /// How long typing rests before the draft is saved on the host.
    private static let rest: Duration = .milliseconds(600)

    init(
        core: String, agentId: String, held: @escaping () -> ChatDraft?, live: @escaping () -> Connection?,
        emit: @escaping () -> Void
    ) {
        self.agentId = agentId
        key = "\(core)-\(agentId)"
        self.held = held
        self.live = live
        self.emit = emit
        guard let unsaved = DraftStore.load(key, as: DraftStore.Unsaved.self) else { return }
        text = unsaved.text
        attachments = unsaved.attachments.filter {
            $0.path != nil || FileManager.default.fileExists(atPath: $0.uri.path)
        }
        tokens = Mentions.tokens(unsaved.text, unsaved.tokens)
        tracked = unsaved.tracked
        chips = unsaved.chips
        rev = unsaved.rev
        edits = 1
    }

    /// Answers whether the draft named anything before, so its chips must be drawn again.
    @discardableResult
    func write(_ next: String) -> Bool {
        let named = !tokens.isEmpty
        let tagged = Set(tokens.map(\.id).filter { $0.hasPrefix(Mentions.image) })
        (text, tokens) = Mentions.edit(text, next, tokens)
        // Deleting a picture's tag takes the picture off the message, as on the Mac.
        let kept = Set(tokens.map(\.id))
        for id in tagged.subtracting(kept) {
            detach(String(id.dropFirst(Mentions.image.count)))
        }
        edited()
        return named
    }

    /// Writes what was picked from the mention menu over the word from `start` that asked for it, as a chip when it
    /// is one.
    func mention(_ start: Int, written: String, id: String? = nil, label: String? = nil, context: ChatContext? = nil) {
        let before = text.units(0, start)
        var kept = tokens.filter { $0.end <= start }
        if let id { kept.append(TextToken(start: start, end: start + written.utf16.count, id: id, label: label)) }
        tokens = kept
        if let context, id == nil, !chips.contains(where: { $0.uri == context.uri }) { chips.append(context) }
        text = written.isEmpty ? before : "\(before)\(written) "
        edited()
    }

    /// Keeps what the issue or pull request chip `id` sends, once the host has read it.
    func track(_ id: String, _ context: ChatContext) {
        guard tokens.contains(where: { $0.id == id }) else { return }
        tracked[id] = context
        edited()
    }

    /// Adds what was picked, leaving out what the host would refuse, and names each new picture in the words at
    /// `caret` as `[Image #n]`, where the host puts it; answers why some was left out.
    func attach(_ picked: [Attachment], at caret: Int? = nil) -> String? {
        let (fits, notice) = Uploads.fitting(held: attachments.count, picked)
        var tags: [(written: String, id: String, label: String)] = []
        for file in fits {
            attachments.append(file)
            if file.kind == .image {
                let number = attachments.count
                tags.append(("[Image #\(number)]", Mentions.image + file.id, "Image \(number)"))
            }
        }
        if !tags.isEmpty { insert(tags, at: min(caret ?? (text as NSString).length, (text as NSString).length)) }
        edited()
        return notice
    }

    /// Writes the pictures' tags at `at`, spaced from the words around them, and keeps them as chips.
    private func insert(_ tags: [(written: String, id: String, label: String)], at: Int) {
        let before = text.units(0, at)
        let after = text.units(at, (text as NSString).length)
        let lead = before.last.map { $0.isWhitespace } ?? true ? "" : " "
        let trail = after.first?.isWhitespace ?? false ? "" : " "
        var start = at + lead.utf16.count
        var placed: [TextToken] = []
        for (index, tag) in tags.enumerated() {
            if index > 0 { start += 1 }
            placed.append(TextToken(start: start, end: start + tag.written.utf16.count, id: tag.id, label: tag.label))
            start += tag.written.utf16.count
        }
        let written = lead + tags.map(\.written).joined(separator: " ") + trail
        let moved = Mentions.edit(text, before + written + after, tokens).tokens
        text = before + written + after
        tokens = (moved + placed).sorted { $0.start < $1.start }
    }

    func removeChip(_ uri: String) {
        chips.removeAll { $0.uri == uri }
        edited()
    }

    /// Answers whether it could, which it cannot while a file is going up.
    func removeAttachment(_ id: String) -> Bool {
        if uploading { return false }
        detach(id)
        edited()
        return true
    }

    /// Takes the file `id` off the message with its tags, and counts the later pictures down so their tags still
    /// name them.
    private func detach(_ id: String) {
        attachments.removeAll { $0.id == id }
        var next = text
        var kept: [TextToken] = []
        var shift = 0
        for token in tokens.sorted(by: { $0.start < $1.start }) {
            var token = token
            token.start += shift
            token.end += shift
            let range = NSRange(location: token.start, length: token.end - token.start)
            if token.id == Mentions.image + id {
                next = (next as NSString).replacingCharacters(in: range, with: "")
                shift -= range.length
                continue
            }
            if token.id.hasPrefix(Mentions.image),
                let number = attachments.firstIndex(where: { Mentions.image + $0.id == token.id }).map({ $0 + 1 })
            {
                let written = "[Image #\(number)]"
                next = (next as NSString).replacingCharacters(in: range, with: written)
                shift += written.utf16.count - range.length
                token.end = token.start + written.utf16.count
                token.label = "Image \(number)"
            }
            kept.append(token)
        }
        text = next
        tokens = kept
    }

    var uploading: Bool { attachments.contains { $0.upload == .sending } }

    func patch(_ id: String, _ change: (inout Attachment) -> Void) {
        guard let at = attachments.firstIndex(where: { $0.id == id }) else { return }
        change(&attachments[at])
        emit()
    }

    /// Takes the files and chips out for the message `text`, answering it with its text chips written in.
    func takeOut(_ message: String) -> (text: String, files: [Attachment], named: Named) {
        let files = attachments
        attachments = []
        var named = Mentions.outgoing(text, tokens, tracked)
        named.context += chips.filter { $0.text.isEmpty }
        let written = Self.withContext(message, chips.filter { !$0.text.isEmpty })
        chips = []
        tokens = []
        tracked = [:]
        edited()
        return (written, files, named)
    }

    /// Puts `texts` and `files` back before what the composer holds now.
    func putBack(_ texts: [String], _ files: [Attachment]) {
        text = (texts + [text]).filter { !$0.isEmpty }.joined(separator: "\n\n")
        tokens = []
        attachments = files + attachments
    }

    /// Empties the draft once the host holds it elsewhere, such as in the stash.
    func clear() {
        text = ""
        attachments = []
        chips = []
        tokens = []
        tracked = [:]
    }

    /// Saves the draft on the host now, answering whether everything written here is saved.
    func saveNow() async -> Bool {
        saving?.cancel()
        await save()
        return edits == 0
    }

    /// The draft changed here: it is saved on the host once typing rests.
    func edited() {
        edits += 1
        saving?.cancel()
        saving = Task { [weak self] in
            try? await Task.sleep(for: Self.rest)
            guard let self, !Task.isCancelled else { return }
            self.keep()
            await self.save()
        }
    }

    /// Writes down what is not saved on the host yet, so it outlives the app quitting.
    func keep() {
        guard edits > 0 else { return }
        DraftStore.save(
            key,
            DraftStore.Unsaved(
                text: text, attachments: attachments.filter { $0.upload == nil }, chips: chips,
                tokens: Mentions.kept(text, tokens), tracked: liveTracked, rev: rev
            ))
    }

    /// Saves the draft as the chat's own on the host, naming the revision last taken in, so the host keeps whatever it
    /// put in the draft since. A save that fails waits for the next change or attach.
    func save() async {
        guard edits > 0, let connection = live() else { return }
        let edits = self.edits
        let paths = Uploads.paths(attachments)
        let extra = DraftExtra(
            tokens: tokens.isEmpty ? nil : Mentions.kept(text, tokens),
            attachments: paths.isEmpty ? nil : paths,
            context: chips.isEmpty ? nil : chips,
            tracked: liveTracked.isEmpty ? nil : liveTracked
        )
        let draft = SetDraftParams(key: "\(WireURIs.chatDraft)\(agentId)", text: text, extra: extra, rev: rev)
        do {
            _ = try await connection.call(PhoneCall.SetDraft.self, draft)
        } catch {
            log.error("draft not saved, kept for the next change: \(problem(error), privacy: .public)")
            return
        }
        guard self.edits == edits else { return }
        self.edits = 0
        DraftStore.clear(key)
        takeIn()
    }

    private var liveTracked: [String: ChatContext] {
        tracked.filter { entry in tokens.contains { $0.id == entry.key } }
    }

    /// Takes in the host's draft, unless changes made here are still to be saved over it.
    func takeIn() {
        guard edits == 0 else { return }
        let host = held()
        if let host, host.rev == rev { return }
        let extra = host?.extra
        let kept = Dictionary(
            attachments.compactMap { attachment in attachment.path.map { ($0, attachment) } },
            uniquingKeysWith: { a, _ in a })
        rev = host?.rev
        text = host?.text ?? ""
        tokens = Mentions.tokens(text, extra?.tokens)
        tracked = extra?.tracked ?? [:]
        chips = extra?.context ?? []
        attachments = (extra?.attachments ?? []).map { kept[$0] ?? .onHost($0) } + attachments.filter { $0.path == nil }
        emit()
    }

    /// `text` with the words of each text chip after it, as the host reads them.
    static func withContext(_ text: String, _ chips: [ChatContext]) -> String {
        ([text] + chips.map(\.text)).filter { !$0.isEmpty }.joined(separator: "\n\n")
    }
}

/// Writing not saved on the host yet, or a new chat's not started, kept on this phone so it outlives the app quitting.
enum DraftStore {
    struct Unsaved: Codable {
        var text: String
        var attachments: [Attachment]
        var chips: [ChatContext]
        var tokens: [DraftToken]
        var tracked: [String: ChatContext]
        var rev: Int?
    }

    private static var folder: URL {
        URL.applicationSupportDirectory.appending(path: "drafts", directoryHint: .isDirectory)
    }

    private static func file(_ key: String) -> URL {
        folder.appending(path: "\(key.addingPercentEncoding(withAllowedCharacters: .alphanumerics) ?? key).json")
    }

    static func load<Value: Decodable>(_ key: String, as kind: Value.Type) -> Value? {
        let file = file(key)
        guard FileManager.default.fileExists(atPath: file.path) else { return nil }
        do {
            return try JSONDecoder().decode(kind, from: Data(contentsOf: file))
        } catch {
            log.error("unsaved draft unreadable: \(problem(error), privacy: .public)")
            return nil
        }
    }

    static func save<Value: Encodable>(_ key: String, _ unsaved: Value) {
        do {
            try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
            try JSONEncoder().encode(unsaved).write(to: file(key), options: .atomic)
        } catch {
            log.error("unsaved draft not kept: \(problem(error), privacy: .public)")
        }
    }

    static func clear(_ key: String) {
        let file = file(key)
        guard FileManager.default.fileExists(atPath: file.path) else { return }
        do {
            try FileManager.default.removeItem(at: file)
        } catch {
            log.error("saved draft not cleared: \(problem(error), privacy: .public)")
        }
    }
}
