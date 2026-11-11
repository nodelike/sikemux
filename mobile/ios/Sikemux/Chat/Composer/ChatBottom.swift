import SwiftUI

/// The bottom of a chat: the composer with what waits on the person docked above it, and the recent prompts.
struct ChatBottom: View {
    let session: ChatSession
    let context: ChatBottomContext
    @ObservedObject private var focus: ComposerFocus

    init(session: ChatSession, context: ChatBottomContext) {
        self.session = session
        self.context = context
        focus = context.focus
    }

    var body: some View {
        let rows = session.rows
        let questions = ChatQuestions.of(context.core, context.agentId)
        let composing = context.watchOnly ? nil : questions.composing
        let info = Hub.shared.live(context.core).view?.chats.first { $0.agentId == context.agentId }
        let recalls = !context.watchOnly && (!rows.queued.isEmpty || rows.messages.contains { $0.role == .user })
        Composer(
            session: session,
            core: context.core,
            provider: context.provider,
            running: rows.running && !session.stoppingTurn,
            steers: context.steers,
            placeholder: composing.map { "Answer @\($0.name)" } ?? placeholder(rows),
            modeLocked: rows.running || !session.permissions.isEmpty || rows.head.connection != .ready || context.offline,
            watchOnly: context.watchOnly,
            offline: context.offline,
            hostName: context.hostName,
            onSent: context.onSent,
            strip: context.strip,
            onRecent: recalls ? { focus.recent = true } : nil,
            question: composing.map { asked in
                ComposerQuestion(
                    name: asked.name, cli: asked.cli, text: asked.text,
                    onRemove: questions.stopComposing,
                    onAnswer: { written in questions.hold(asked, written) { session.setDraft(written.answer) } }
                )
            },
            focus: focus
        )
        .liftedSheet(shown: $focus.recent) {
            RecentSheet(prompts: focus.recent ? RecentSheet.sent(rows.messages, queued: rows.queued.map(\.text)) : []) { text in
                session.setDraft(text)
                focus.recent = false
            }
        }
        .onAppear { questions.report = session.report }
        .onChange(of: "\(info?.activeAt ?? 0)|\(info?.unread ?? false)|\(rows.running)", initial: true) { _, seen in
            questions.read(seen)
        }
    }

    private func placeholder(_ rows: ChatRows) -> String {
        if context.offline { return "Reconnect to continue this conversation" }
        if rows.running { return context.steers ? "Send to steer this turn" : "Send to queue behind the running turn" }
        return rows.commands.isEmpty ? "Ask about this project" : "Ask about this project, or type / for commands"
    }
}

/// The files a sent message carried, as this phone picked them or as the host keeps them.
struct SentAttachments: View {
    @Environment(\.palette) private var palette
    @Environment(TranscriptContext.self) private var transcript
    let paths: [String]

    var body: some View {
        let sent = transcript.core.flatMap { core in transcript.agentId.map { ChatSession.of(core: core, agentId: $0).sentFiles } } ?? [:]
        FlowRight(spacing: 6) {
            ForEach(paths, id: \.self) { path in
                let picked = sent[path]
                let name = picked?.name ?? (path as NSString).lastPathComponent
                if picked.map({ $0.kind == .image }) ?? (HostFiles.imagePath(path) != nil) {
                    SentImage(path: path, picked: picked, name: name)
                } else {
                    FileChip(name: name, size: picked?.size)
                        .accessibilityElement(children: .ignore)
                        .accessibilityLabel("File \(name)")
                }
            }
        }
        .frame(maxWidth: UIScreen.main.bounds.width * 0.84, alignment: .trailing)
        .padding(.bottom, 6)
    }
}

/// A picture a message carried: a thumbnail once it is on this phone, and the picture over the whole screen on a tap.
private struct SentImage: View {
    @Environment(\.palette) private var palette
    @Environment(TranscriptContext.self) private var transcript
    let path: String
    let picked: Attachment?
    let name: String
    @State private var image: UIImage?

    var body: some View {
        Button {
            if let image { transcript.picture = ShownPicture(image: image, name: name) }
        } label: {
            if let image {
                Image(uiImage: image)
                    .resizable()
                    .scaledToFill()
                    .frame(width: 132, height: 99)
                    .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
                    .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(palette.border, lineWidth: 1))
            } else {
                FileChip(name: name, size: picked?.size, image: true)
            }
        }
        .buttonStyle(PressFade())
        .accessibilityLabel("\(name), open")
        .task(id: path) {
            let local = picked.flatMap { $0.onHostOnly ? nil : $0.uri }
            var url = local
            if url == nil, let core = transcript.core { url = await HostFiles.shared.read(core: core, path: path) }
            guard let url else { return }
            image = await Task.detached { UIImage(contentsOfFile: url.path) }.value
        }
    }
}
