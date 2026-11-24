import SwiftUI

/// The host's chats searched by their agents' names, their titles and what was said in them, in the order the host
/// ranks them, each found by its words with the line that holds them.
struct ChatSearch: View {
    @Environment(\.palette) private var palette
    let core: String
    let view: DeviceView
    let scope: ProjectInfo?
    let full: Bool
    let hostName: String
    let onClose: () -> Void

    @State private var query = ""
    @State private var found: Found?
    @State private var resuming: String?
    @State private var note: (text: String, failed: Bool)?
    @FocusState private var typing: Bool
    /// How long typing rests before the host is asked.
    private static let typingRest: Duration = .milliseconds(150)
    private static let most = 50

    var body: some View {
        let words = query.trimmingCharacters(in: .whitespaces).lowercased()
        let shown = found?.words == words ? found : nil
        let said = shown?.said ?? [:]
        let agentOf = Dictionary(
            (view.inbox.agents + (view.inbox.kept ?? [])).map { ($0.agentId, $0) },
            uniquingKeysWith: { first, _ in first })
        let rows: [Hit] = (shown?.found ?? []).compactMap { found in
            if let agentId = found.agentId {
                guard let agent = agentOf[agentId], agent.session == nil else { return nil }
                return .agent(agent, found)
            }
            return found.chat.flatMap { shown?.chats[$0] }.map { .saved($0, found) }
        }
        let named = rows.prefix { $0.found.by != .said }
        let byWords = rows.dropFirst(named.count)

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
                    ForEach([Array(named), Array(byWords)].filter { !$0.isEmpty }, id: \.first?.id) { group in
                        Rows {
                            ForEach(group, id: \.id) { hit in
                                hitRow(hit, line: hit.found.chat.flatMap { said[$0] })
                            }
                        }
                        .padding(.bottom, 14)
                    }
                    if let note {
                        Text(note.text)
                            .font(Theme.ui(12.5))
                            .foregroundStyle(note.failed ? palette.danger : palette.tertiary)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(.horizontal, 8)
                            .padding(.top, 8)
                    }
                    if !words.isEmpty && shown?.complete == true && rows.isEmpty {
                        EmptyState(icon: Icons.search, text: String(localized: "No chats match."))
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

    @ViewBuilder
    private func hitRow(_ hit: Hit, line: FoundLine?) -> some View {
        switch hit {
        case .agent(let agent, _):
            let title = agent.chief == true ? CallWords.chiefRole : agent.title
            Row(
                title: title,
                name: agent.name,
                underText: line.map { FoundLineText.of($0, palette) },
                onPress: { open(agent.agentId, line) },
                mark: {
                    OwnMark(
                        agentId: agent.agentId, provider: agent.cli, chief: agent.chief == true,
                        size: 20, still: true)
                }, end: { EmptyView() })
        case .saved(let chat, _):
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

    /// Asks the host at once for what its names and titles find, then once typing rests for what was said too,
    /// whose rows go under those already shown.
    private func look(for words: String) async {
        guard !words.isEmpty else {
            found = nil
            return
        }
        guard let connection = Hub.shared.live(core).connection else { return }
        let quick = RecentChatsParams(project: scope?.id, query: words, cursor: nil, limit: Self.most, quick: true)
        if let answer = try? await connection.call(PhoneCall.RecentChats.self, quick), !Task.isCancelled {
            found = Found(words: words, answer: answer, complete: words.hasPrefix("@"))
        }
        guard !words.hasPrefix("@") else { return }
        try? await Task.sleep(for: Self.typingRest)
        guard !Task.isCancelled else { return }
        let full = RecentChatsParams(project: scope?.id, query: words, cursor: nil, limit: Self.most, quick: false)
        let answer = try? await connection.call(PhoneCall.RecentChats.self, full)
        guard !Task.isCancelled, let answer else { return }
        found = (found?.words == words ? found : nil).map { $0.adding(answer) } ?? Found(words: words, answer: answer)
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
            note = (String(localized: "This phone can only watch \(hostName)."), false)
            return
        }
        let live = Hub.shared.live(core)
        guard live.isOpen, let connection = live.connection else {
            Haptics.failure()
            note = (String(localized: "Not connected to the host."), true)
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

/// What the host found for the words typed: its rows in the order it sent them, the saved chats they name, and where
/// each chat said the words.
private struct Found {
    let words: String
    var found: [FoundChat]
    var chats: [String: RecentInfo]
    var said: [String: FoundLine]
    /// Every answer has come, so finding nothing means nothing matches.
    var complete = true

    init(words: String, answer: RecentChats, complete: Bool = true) {
        self.words = words
        found = answer.found ?? []
        chats = Dictionary(answer.chats.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        said = answer.said ?? [:]
        self.complete = complete
    }

    /// The later answer's rows go under these, leaving every row already here where it is.
    func adding(_ answer: RecentChats) -> Found {
        var more = self
        let here = Set(found.map(\.key))
        more.found += (answer.found ?? []).filter { !here.contains($0.key) }
        more.chats.merge(answer.chats.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        more.said.merge(answer.said ?? [:], uniquingKeysWith: { _, later in later })
        more.complete = true
        return more
    }
}

extension FoundChat {
    fileprivate var key: String { agentId ?? chat ?? "" }
}

/// A row the search found: an open agent, or a saved chat to take up again.
private enum Hit {
    case agent(AgentShown, FoundChat)
    case saved(RecentInfo, FoundChat)

    var found: FoundChat {
        switch self {
        case .agent(_, let found), .saved(_, let found): found
        }
    }

    var id: String { found.key }
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
