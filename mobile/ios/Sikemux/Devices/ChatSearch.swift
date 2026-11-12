import SwiftUI

/// The host's chats searched by their titles and by what was said in them: the open agents narrowed here, and the
/// saved chats the host finds, each found by its words with the line that holds them.
struct ChatSearch: View {
    @Environment(\.palette) private var palette
    let core: String
    let view: DeviceView
    let scope: ProjectInfo?
    let full: Bool
    let hostName: String
    let onClose: () -> Void

    @State private var query = ""
    @State private var found: RecentChats?
    @State private var resuming: String?
    @State private var note: (text: String, failed: Bool)?
    @FocusState private var typing: Bool
    /// How long typing rests before the host is asked.
    private static let typingRest: Duration = .milliseconds(150)
    private static let most = 50

    var body: some View {
        let words = query.trimmingCharacters(in: .whitespaces).lowercased()
        let said = found?.said ?? [:]
        let chatOf = Dictionary(
            view.recent.compactMap { chat in chat.agentId.map { ($0, chat.id) } },
            uniquingKeysWith: { first, _ in first })
        let agents =
            words.isEmpty
            ? []
            : view.inbox.agents.filter { agent in
                (scope == nil || agent.project == scope?.id) && agent.session == nil
                    && (agent.title.lowercased().contains(words)
                        || chatOf[agent.agentId].map { said[$0] != nil } == true)
            }
        let saved = (found?.chats ?? []).filter { $0.agentId == nil }

        VStack(spacing: 0) {
            HStack(spacing: 12) {
                HStack(spacing: 8) {
                    IconView(icon: Icons.search, size: 15).foregroundStyle(palette.tertiary)
                    TextField("", text: $query, prompt: Text("Search chats…").foregroundStyle(palette.tertiary))
                        .font(Theme.ui(15.5))
                        .foregroundStyle(palette.ink)
                        .tint(palette.accent)
                        .autocorrectionDisabled()
                        .textInputAutocapitalization(.never)
                        .submitLabel(.search)
                        .focused($typing)
                }
                .padding(.horizontal, 12)
                .frame(height: 40)
                .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(palette.sunken))
                .overlay(
                    RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(
                        typing ? palette.borderSelected : palette.border, lineWidth: 1))
                Button("Cancel", action: onClose)
                    .font(Theme.ui(15.5))
                    .foregroundStyle(palette.accent)
            }
            .padding(.horizontal, 16)
            .padding(.top, 6)
            .padding(.bottom, 10)
            ScrollView(showsIndicators: false) {
                VStack(spacing: 0) {
                    if !agents.isEmpty {
                        Rows {
                            ForEach(agents, id: \.agentId) { agent in
                                let line = chatOf[agent.agentId].flatMap { said[$0] }
                                Row(
                                    title: agent.title, name: agent.name,
                                    underText: line.map { FoundLineText.of($0, palette) },
                                    onPress: { open(agent.agentId, line) },
                                    mark: { AgentMark(provider: agent.cli, size: 20) }, end: { EmptyView() })
                            }
                        }
                    }
                    if !saved.isEmpty {
                        SectionLabel("Recent")
                        Rows {
                            ForEach(saved, id: \.id) { chat in
                                let line = said[chat.id]
                                Row(
                                    title: chat.title, underText: line.map { FoundLineText.of($0, palette) },
                                    onPress: { resume(chat, line) },
                                    mark: { AgentMark(provider: chat.provider, size: 20) },
                                    end: {
                                        if resuming == chat.id {
                                            Working()
                                        } else {
                                            Text(Format.age(Format.date(millis: chat.activeAt), now: .now))
                                                .font(Theme.ui(12)).monospacedDigit().foregroundStyle(palette.tertiary)
                                        }
                                    })
                            }
                        }
                    }
                    if let note {
                        Text(note.text)
                            .font(Theme.ui(12.5))
                            .foregroundStyle(note.failed ? palette.danger : palette.tertiary)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(.horizontal, 8)
                            .padding(.top, 8)
                    }
                    if !words.isEmpty && found != nil && agents.isEmpty && saved.isEmpty {
                        EmptyState(icon: Icons.search, text: "No chats match.")
                    }
                }
                .padding(.horizontal, 16)
                .padding(.bottom, 24)
            }
            .scrollDismissesKeyboard(.interactively)
        }
        .onAppear { typing = true }
        .task(id: words) { await look(for: words) }
    }

    private func look(for words: String) async {
        guard !words.isEmpty else {
            found = nil
            return
        }
        try? await Task.sleep(for: Self.typingRest)
        guard !Task.isCancelled, let connection = Hub.shared.live(core).connection else { return }
        let params = RecentChatsParams(project: scope?.id, query: words, cursor: nil, limit: Self.most)
        let answer = try? await connection.call(PhoneCall.RecentChats.self, params)
        guard !Task.isCancelled else { return }
        found = answer
    }

    private func open(_ agentId: String, _ line: FoundLine?) {
        Haptics.tap()
        PendingFind.ask(core: core, agent: agentId, line: line)
        Router.shared.push(.chat(core: core, agent: agentId))
    }

    private func resume(_ chat: RecentInfo, _ line: FoundLine?) {
        guard resuming == nil else { return }
        guard full else {
            Haptics.warning()
            note = ("This phone can only watch \(hostName).", false)
            return
        }
        let live = Hub.shared.live(core)
        guard live.isOpen, let connection = live.connection else {
            Haptics.failure()
            note = ("Not connected to the host.", true)
            return
        }
        Haptics.tap()
        note = nil
        resuming = chat.id
        Task {
            defer { resuming = nil }
            do {
                let agentId = try await connection.resumeChat(recent: chat.id)
                PendingFind.ask(core: core, agent: agentId, line: line)
                Router.shared.push(.chat(core: core, agent: agentId))
            } catch {
                Haptics.failure()
                note = (problem(error), true)
            }
        }
    }
}

/// The line a chat said the words in: the quiet ink, the words themselves in full ink, as the host cut it.
enum FoundLineText {
    static func of(_ line: FoundLine, _ palette: Palette) -> Text {
        line.runs.reduce(Text("")) { text, run in
            run.found == true
                ? text + Text(run.text).foregroundColor(palette.ink).fontWeight(.medium)
                : text + Text(run.text)
        }
    }
}

/// The words a chat opened from a search finds once it shows, so it opens on them.
@MainActor
enum PendingFind {
    private static var asked: (core: String, agent: String, words: String, at: Date)?
    /// A chat that never showed lets its find go, so a later plain open does not find.
    private static let lapse: TimeInterval = 30

    static func ask(core: String, agent: String, line: FoundLine?) {
        asked = line.flatMap { line in
            line.runs.first { $0.found == true }.map { (core, agent, $0.text, Date.now) }
        }
    }

    /// The words to find in `agent`'s chat, once.
    static func take(core: String, agent: String) -> String? {
        guard let asked, asked.core == core, asked.agent == agent else { return nil }
        self.asked = nil
        return Date.now.timeIntervalSince(asked.at) < lapse ? asked.words : nil
    }
}
