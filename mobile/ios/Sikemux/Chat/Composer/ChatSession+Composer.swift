import Foundation
import SikemuxCore

/// What the composer and the docks above it ask of a chat.
extension ChatSession {
    var draftText: String { draft.text }

    func setDraft(_ text: String) {
        guard text != draft.text else { return }
        draft.write(text)
    }

    /// Writes what was picked from the mention menu over the word from `start` that asked for it, as a chip when it
    /// is one.
    func mention(_ start: Int, written: String, id: String? = nil, label: String? = nil, context: ChatContext? = nil) {
        draft.mention(start, written: written, id: id, label: label, context: context)
    }

    /// Keeps what the issue or pull request chip `id` sends, once the host has read it.
    func track(_ id: String, _ context: ChatContext) {
        draft.track(id, context)
    }

    /// Adds what was picked for the next message, leaving out what the host would refuse.
    func attach(_ picked: [Attachment]) {
        notice = draft.attach(picked)
    }

    /// Has the host put stash items in this chat's draft: files beside it, links and text as chips, notes in its words.
    func attachStash(_ ids: [String]) async throws {
        let connection = try openConnection()
        for id in ids {
            _ = try await connection.call(PhoneCall.AttachStash.self, AttachStashParams(id: id, agentId: agentId))
        }
    }

    func removeChip(_ uri: String) {
        draft.removeChip(uri)
    }

    func removeAttachment(_ id: String) {
        _ = draft.removeAttachment(id)
    }

    /// Stashes the whole draft as one prompt, with the files picked here put on the host first, and empties it.
    /// Answers the prompt's id.
    func stashPrompt() async throws -> String {
        let connection = try openConnection()
        if draft.attachments.contains(where: { $0.path == nil }) {
            draft.attachments = try await outbox.put(connection, draft.attachments) { id, change in self.draft.patch(id, change) }
            draft.edited()
        }
        guard await draft.saveNow() else { throw OutboxError.failed("the draft did not reach the host") }
        let stashed = try await connection.call(PhoneCall.StashPrompt.self, StashPromptParams(agentId: agentId))
        draft.clear()
        return stashed.id
    }

    /// Puts a stashed prompt back in the draft and takes it out of the stash.
    func unstashPrompt(_ id: String) async throws {
        let connection = try openConnection()
        _ = try await connection.call(PhoneCall.AttachStash.self, AttachStashParams(id: id, agentId: agentId))
        _ = try await connection.call(PhoneCall.RemoveStash.self, StashItemParams(id: id))
    }

    /// Shows the message at once, behind the running turn when there is one or into it when `steers`, and puts its
    /// files on the host before sending it. Answers whether the composer can let it go.
    func send(_ text: String, steers: Bool = false) -> Bool {
        if draft.uploading { return false }
        notice = nil
        let message = draft.takeOut(text)
        if outbox.waiting {
            outbox.deliver(message.text, message.files, waiting: true, named: message.named)
        } else if busy || outbox.sending {
            queue(message.text, message.files, steers: steers && feed.chat.running, named: message.named)
        } else {
            outbox.deliver(message.text, message.files, named: message.named)
        }
        return true
    }

    /// Shows a message begun on another screen at once, with its files already on the host, and sends it once the
    /// chat is attached.
    func sendWhenAttached(_ text: String, _ uploaded: [Attachment]) {
        outbox.deliver(text, uploaded, waiting: true)
    }

    /// Answers a question with `text` and the files picked for it, named in the answer once they are all on the host.
    /// The files stay in the composer when the answer did not go out.
    func answerWith(_ text: String, _ answer: (WrittenAnswer) async -> Bool) async -> Bool {
        notice = nil
        var attachments: [Attachment] = []
        if !draft.attachments.isEmpty {
            guard !draft.uploading else { return false }
            do {
                attachments = try await outbox.put(connection, draft.attachments) { id, change in self.draft.patch(id, change) }
            } catch {
                notice = Sikemux.problem(error)
                return false
            }
            draft.attachments = []
        }
        if await answer(WrittenAnswer(answer: text, paths: Uploads.paths(attachments), context: draft.chips)) {
            draft.chips = []
            return true
        }
        draft.putBack([], attachments)
        return false
    }

    /// Puts waiting messages into the running turn now, as one, instead of after it.
    func steer(_ ids: [String]) {
        let ids = outbox.onHostQueue(ids)
        guard !ids.isEmpty else { return }
        Task {
            await ask("Could not steer") { connection in
                try await connection.call(PhoneCall.SteerQueued.self, QueuedParams(agentId: self.agentId, ids: ids))
            }
        }
    }

    func dropQueued(_ id: String) {
        guard !outbox.onHostQueue([id]).isEmpty else { return }
        Task {
            await ask("Could not take it back") { connection in
                try await connection.call(PhoneCall.DropQueued.self, QueuedParams(agentId: self.agentId, ids: [id]))
            }
        }
    }

    /// Stops the running turn. Messages waiting behind it go back to the composer rather than out.
    func cancel() {
        let chat = feed.chat
        if !chat.queued.isEmpty {
            let texts = chat.queued.map(\.text).filter { !$0.isEmpty }
            var seen = Set<String>()
            let paths = chat.queued.flatMap(\.paths).filter { seen.insert($0).inserted }
            let sent = paths.compactMap { path in outbox.sentFiles[path].map { file -> Attachment in
                var file = file
                file.path = path
                return file
            } }
            draft.putBack(texts, sent)
            draft.edited()
        }
        notice = nil
        if chat.running { stoppingTurn = true }
        Task {
            await ask("Could not stop the turn", undo: { self.stoppingTurn = false }) { connection in
                try await connection.cancel(agentId: self.agentId)
            }
        }
    }

    /// Answers a permission request with one of its options, or turns it down with none.
    func answer(_ requestId: String, _ optionId: String?) {
        guard !answering.contains(requestId) else { return }
        answering.insert(requestId)
        notice = nil
        Task {
            await ask("The answer did not reach the host", then: { _ in
                self.feed.chat.permissions.removeAll { $0.requestId == requestId }
            }) { connection in
                try await connection.answerPermission(agentId: self.agentId, requestId: requestId, optionId: optionId)
            }
            answering.remove(requestId)
        }
    }

    /// `bypass` runs without asking; `workspaceWrite` asks first.
    func setPermissionMode(_ chosen: PermissionMode) {
        if feed.chat.changingMode { return }
        let mode = chosen.rawValue
        let before = self.mode
        self.mode = mode
        notice = nil
        Task {
            await ask("Could not change permissions", undo: { if self.mode == mode { self.mode = before } }) { connection in
                try await connection.setPermissionMode(agentId: self.agentId, mode: mode)
            }
        }
    }

    /// Shows the pick at once, and goes back to what the host runs on if it refuses it.
    func setConfig(_ configId: String, _ value: String) {
        notice = nil
        chosen[configId] = value
        Task {
            await ask("Could not change the setting", undo: {
                if self.chosen[configId] == value { self.chosen[configId] = nil }
            }) { connection in
                _ = try await connection.setChatConfig(agentId: self.agentId, configId: configId, value: value)
            }
        }
    }

    /// Sends behind the running turn; a message that does not go goes back to the composer.
    private func queue(_ text: String, _ files: [Attachment], steers: Bool, named: Named) {
        Task {
            do {
                try await outbox.queue(text, files, steers: steers, named: named)
            } catch {
                draft.putBack([text], files)
                notice = "Could not send the message: \(Sikemux.problem(error))"
            }
        }
    }
}

/// What the person wrote for a question and what they sent with it, which the host writes out as one text.
struct WrittenAnswer: Sendable {
    var answer: String
    var paths: [String] = []
    var context: [ChatContext] = []
}
