import OSLog
import SikemuxCore
import SwiftUI

private let log = Logger(subsystem: "com.nodelike.sikemux", category: "mentions")

/// One thing the menu offers, and what taking it writes into the draft.
struct MentionRow: Identifiable {
    enum Mark {
        case agent(id: String?, provider: String)
        case icon(IconDef)
    }

    let id: String
    let title: String
    /// The title is words, such as a chat's, rather than a handle to type.
    var plain = false
    var detail: String?
    var aside: String?
    /// The heading it starts, where the rows above it belong to another.
    var group: String?
    let mark: Mark
    var state: AgentGroup?
    let take: () -> Void
}

/// What the menu over the composer offers for the word the draft ends with: the host's other agents, closed and
/// earlier chats for `@`, the project's files and folders for `+`, its issues for `#` and pull requests for `!`.
@MainActor @Observable
final class MentionSearch {
    private struct Item: Decodable {
        let kind: TrackedKind
        let number: Int
        let title: String
        let url: String
    }

    private enum Listed {
        case items([Item])
        case failed(String)
    }

    private static let shown = 8
    private static let earlierShown = 4
    private static let earlierBudget = 600
    /// How long typing must rest before the host searches earlier chats.
    private static let settle: Duration = .milliseconds(150)

    let core: String
    let session: ChatSession
    private var closed: (cwd: String, list: [ClosedAgent])?
    private var found: (query: String, list: [SessionMatch])?
    private var entries: (query: String, list: [ProjectEntry]?, failed: String?)?
    private var tracked: (cwd: String, listed: Listed)?
    @ObservationIgnored private var asked: (trigger: Typed.Trigger, needle: String, cwd: String?)?
    @ObservationIgnored private var tasks: [String: Task<Void, Never>] = [:]

    init(core: String, session: ChatSession) {
        self.core = core
        self.session = session
    }

    private var live: HostLive { Hub.shared.live(core) }
    private var view: DeviceView? { live.view }
    private var own: WorkspaceAgent? { view?.workspace.agents?.first { $0.id == session.agentId } }
    private var project: ProjectInfo? { view?.project(id: own?.project) }

    /// Asks the host for what `typed` names, once for each word typed.
    func ask(_ typed: Typed?) {
        guard let typed, let connection = live.connection else { return }
        let cwd = project?.path
        if let asked, asked.trigger == typed.trigger, asked.needle == typed.needle, asked.cwd == cwd { return }
        asked = (typed.trigger, typed.needle, cwd)
        guard let cwd else { return }
        switch typed.trigger {
        case .agents:
            if closed?.cwd != cwd {
                run("closed") {
                    do {
                        self.closed = (
                            cwd, try await connection.call(PhoneCall.ClosedAgents.self, ClosedAgentsParams(cwd: cwd))
                        )
                    } catch {
                        log.error("closed agents: \(problem(error), privacy: .public)")
                        self.closed = (cwd, [])
                    }
                }
            }
            let query = typed.needle.trimmingCharacters(in: .whitespaces).lowercased()
            guard !query.isEmpty else { return }
            run("found") {
                try? await Task.sleep(for: Self.settle)
                if Task.isCancelled { return }
                let params = SessionSearchParams(
                    query: query, projects: [cwd], limit: Self.earlierShown, budgetMs: Self.earlierBudget)
                do {
                    let list = try await connection.call(PhoneCall.AgentSessions.self, params)
                    if !Task.isCancelled { self.found = (query, list) }
                } catch {
                    log.error("earlier chats: \(problem(error), privacy: .public)")
                    if !Task.isCancelled { self.found = (query, []) }
                }
            }
        case .files:
            let needle = typed.needle
            run("entries") {
                do {
                    let list = try await connection.call(
                        PhoneCall.ProjectEntries.self,
                        ProjectEntriesParams(project: cwd, query: needle, limit: Self.shown))
                    if !Task.isCancelled { self.entries = (needle, list, nil) }
                } catch {
                    if !Task.isCancelled { self.entries = (needle, nil, problem(error)) }
                }
            }
        case .issues, .pulls:
            guard let host = project?.codeHost?.label, tracked?.cwd != cwd else { return }
            run("tracked") {
                do {
                    let answer = try await connection.call(
                        PhoneCall.CodeHostTracked.self, CodeHostProjectParams(project: cwd))
                    self.tracked = (cwd, Self.read(answer, host))
                } catch {
                    self.tracked = (cwd, .failed(problem(error)))
                }
            }
        }
    }

    private func run(_ name: String, _ work: @escaping @MainActor () async -> Void) {
        tasks[name]?.cancel()
        tasks[name] = Task { await work() }
    }

    private static func read(_ answer: PluginOutcome, _ host: String) -> Listed {
        switch answer {
        case .failed(let failure):
            return .failed(
                failure.category == .auth
                    ? "Sign in to \(host) in the Git pane to pick from its issues" : failure.message)
        case .value(let value):
            do {
                return .items(try value.decoded(as: [Item].self))
            } catch {
                return .failed(problem(error))
            }
        case .unknown:
            return .items([])
        }
    }

    /// The rows for `typed`, and what the menu says while it offers none; nil words while the list is on its way.
    func rows(_ typed: Typed) -> (rows: [MentionRow], empty: String?) {
        guard let view else { return ([], nil) }
        let start = typed.start
        let session = session
        let write = { (written: String, id: String?, label: String?, context: ChatContext?) in
            session.draft.mention(start, written: written, id: id, label: label, context: context)
        }
        let wanted = typed.needle.lowercased()
        let project = project
        let cwd = project?.path

        switch typed.trigger {
        case .agents:
            let agents = view.workspace.agents ?? []
            let groups = Dictionary(view.chats.map { ($0.agentId, $0.group) }, uniquingKeysWith: { a, _ in a })
            let places = [project].compactMap { $0 } + view.workspace.projects.filter { $0.id != project?.id }
            let mates: [MentionRow] = places.flatMap { place in
                agents.filter {
                    $0.project == place.id && $0.id != session.agentId && ($0.name ?? "").lowercased().hasPrefix(wanted)
                }
            }
            .prefix(Self.shown)
            .map { agent in
                let name = agent.name ?? ""
                return MentionRow(
                    id: "\(Mentions.agent)\(agent.id)", title: "@\(name)", detail: agent.title,
                    group: agent.project == project?.id
                        ? nil : view.project(id: agent.project)?.name,
                    mark: .agent(id: agent.id, provider: agent.provider), state: groups[agent.id] ?? nil,
                    take: { write("@\(name)", "\(Mentions.agent)\(agent.id)", nil, nil) }
                )
            }
            let named = (closed?.cwd == cwd ? closed?.list ?? [] : []).filter { $0.name.lowercased().hasPrefix(wanted) }
                .prefix(Self.shown)
            let closedRows: [MentionRow] = named.enumerated().map { index, agent in
                let id = "\(Mentions.earlier)\(agent.provider)/\(agent.sessionId)"
                return MentionRow(
                    id: id, title: "@\(agent.name)", detail: agent.title, group: index == 0 ? "Closed" : nil,
                    mark: .agent(id: nil, provider: agent.provider), take: { write("@\(agent.name)", id, nil, nil) }
                )
            }
            let offered = Set(view.workspace.launchers.map(\.provider))
            let open = Set(agents.compactMap { agent in agent.resumeId.map { "\(agent.provider)/\($0)" } })
            let query = wanted.trimmingCharacters(in: .whitespaces)
            let earlier = (found?.query == query ? found?.list ?? [] : [])
                .filter { offered.contains($0.cli) && !open.contains("\($0.cli)/\($0.sessionId)") }
                .filter { chat in !named.contains { $0.sessionId == chat.sessionId } }
            let earlierRows: [MentionRow] = earlier.enumerated().map { index, chat in
                MentionRow(
                    id: "\(Mentions.earlier)\(chat.cli)/\(chat.sessionId)", title: chat.title, plain: true,
                    aside: Format.age(Format.date(millis: Double(chat.activeAt))),
                    group: index == 0 ? "Earlier chats" : nil,
                    mark: .agent(id: nil, provider: chat.cli),
                    take: {
                        write(
                            "", nil, nil,
                            ChatContext(
                                uri: "\(WireURIs.earlierURI)\(chat.cli)/\(chat.sessionId)", title: chat.title, text: "")
                        )
                    }
                )
            }
            let rows = mates + closedRows + earlierRows
            return (
                rows, rows.isEmpty ? (wanted.isEmpty ? "No other agent yet" : "No agent or earlier chat matches") : nil
            )

        case .files:
            guard let project else { return ([], "Open a project to pick its files") }
            let answered = entries?.query == typed.needle ? entries : nil
            if let failed = answered?.failed { return ([], failed) }
            let draft = session.draftText
            let taken = session.draft.tokens.map { draft.units($0.start, $0.end) }
            let rows: [MentionRow] = (answered?.list ?? []).map { entry in
                let whole = "\(project.path)/\(entry.path)"
                let name = (entry.path as NSString).lastPathComponent
                let written = "+\(taken.contains("+\(name)") ? entry.path : name)"
                let folder = (entry.path as NSString).deletingLastPathComponent
                return MentionRow(
                    id: entry.path, title: name, detail: folder.isEmpty ? "." : folder,
                    mark: .icon(entry.folder ? Icons.folder : Icons.file),
                    take: { write(written, "\(entry.folder ? Mentions.folder : Mentions.file)\(whole)", nil, nil) }
                )
            }
            return (rows, rows.isEmpty && answered != nil ? "Nothing in the project matches" : nil)

        case .issues, .pulls:
            let pulls = typed.trigger == .pulls
            guard let project, let cwd else {
                return ([], "Open a project to pick its \(pulls ? "pull requests" : "issues")")
            }
            guard project.codeHost != nil else { return ([], "This project has no remote on a code host") }
            let listed = tracked?.cwd == cwd ? tracked?.listed : nil
            if case .failed(let why) = listed { return ([], why) }
            var items: [Item] = []
            if case .items(let all) = listed { items = all }
            let query = typed.needle.trimmingCharacters(in: .whitespaces).lowercased()
            let digits = !query.isEmpty && query.allSatisfy(\.isNumber)
            let words = query.split(whereSeparator: \.isWhitespace).map(String.init)
            let connection = live.connection
            let rows: [MentionRow] =
                items
                .filter { $0.kind == (pulls ? .pull : .issue) }
                .filter {
                    digits ? String($0.number).hasPrefix(query) : words.allSatisfy($0.title.lowercased().contains)
                }
                .prefix(Self.shown)
                .map { item in
                    let sign = pulls ? "!" : "#"
                    let id = Mentions.trackedId(item.kind, item.number, item.url)
                    return MentionRow(
                        id: id, title: "\(sign)\(item.number)", detail: item.title,
                        mark: .icon(pulls ? Icons.pullRequest : Icons.issue),
                        take: {
                            write("\(sign)\(item.number)", id, "#\(item.number) \(item.title)", nil)
                            guard let connection else { return }
                            Task {
                                let params = TrackedContextParams(
                                    project: cwd, kind: pulls ? .pull : .issue, number: item.number)
                                do {
                                    let answer = try await connection.call(
                                        PhoneCall.CodeHostTrackedContext.self, params)
                                    guard case .value(let value) = answer else { return }
                                    session.draft.track(id, try value.decoded(as: ChatContext.self))
                                } catch {
                                    log.error(
                                        "what the issue or pull request says: \(problem(error), privacy: .public)")
                                }
                            }
                        }
                    )
                }
            let none = pulls ? "No open pull request matches" : "No open issue matches"
            return (rows, rows.isEmpty && listed != nil ? none : nil)
        }
    }
}

/// The menu above the composer while its last word starts with `@`, `+`, `#` or `!`, under the kinds of thing it
/// names; a tap on a kind asks for that instead, and a tap on a row writes it into the draft.
struct MentionMenu: View {
    @Environment(\.palette) private var palette
    let typed: Typed
    let rows: [MentionRow]
    let empty: String?
    let onScope: (Typed.Trigger) -> Void

    private static let scopes: [(Typed.Trigger, String)] = [
        (.agents, "Agents"), (.issues, "Issues"), (.pulls, "Pull requests"), (.files, "Files"),
    ]

    var body: some View {
        VStack(spacing: 0) {
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 2) {
                    ForEach(Self.scopes, id: \.0) { trigger, label in
                        let on = trigger == typed.trigger
                        Button {
                            guard !on else { return }
                            Haptics.select()
                            onScope(trigger)
                        } label: {
                            HStack(spacing: 5) {
                                Text(trigger.rawValue).font(Theme.mono(12.5)).foregroundStyle(
                                    on ? palette.accent : palette.inkFaint)
                                Text(label).font(Theme.ui(12.5, .medium)).foregroundStyle(
                                    on ? palette.ink : palette.tertiary)
                            }
                            .padding(.horizontal, 9)
                            .frame(height: 30)
                            .background(
                                RoundedRectangle(cornerRadius: 8, style: .continuous).fill(on ? palette.active : .clear)
                            )
                        }
                        .buttonStyle(.plain)
                        .accessibilityAddTraits(on ? [.isSelected] : [])
                    }
                }
                .padding(5)
            }
            palette.border.frame(height: 1)
            if rows.isEmpty {
                Group {
                    if let empty {
                        Text(empty).font(Theme.ui(13)).foregroundStyle(palette.tertiary)
                    } else {
                        ProgressView().tint(palette.tertiary).controlSize(.small)
                    }
                }
                .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
                .padding(.horizontal, 14)
            } else {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 0) {
                        ForEach(rows) { row in
                            if let group = row.group {
                                Text(group.uppercased())
                                    .font(Theme.ui(10.5, .semibold))
                                    .tracking(0.9)
                                    .foregroundStyle(palette.inkFaint)
                                    .padding(.top, 9)
                                    .padding(.bottom, 3)
                                    .padding(.horizontal, 14)
                            }
                            button(row)
                        }
                    }
                    .padding(.vertical, 3)
                }
                .scrollBounceBehavior(.basedOnSize)
                .frame(maxHeight: 232)
                .fixedSize(horizontal: false, vertical: true)
            }
        }
        .background(palette.overlay)
        .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(palette.border, lineWidth: 1))
        .padding(.bottom, 8)
    }

    private func button(_ row: MentionRow) -> some View {
        Button {
            Haptics.select()
            row.take()
        } label: {
            HStack(spacing: 10) {
                Group {
                    switch row.mark {
                    case .icon(let icon): IconView(icon: icon, size: 15).foregroundStyle(palette.tertiary)
                    case .agent(let id, let provider):
                        AgentFaceView(agentId: id, provider: provider, group: nil, size: 18, logoSize: 16)
                    }
                }
                .frame(width: 18)
                Text(row.title)
                    .font(row.plain ? Theme.ui(13) : Theme.mono(13))
                    .foregroundStyle(row.plain ? palette.ink : palette.accent)
                    .lineLimit(1)
                    .layoutPriority(1)
                Text(row.detail ?? "").font(Theme.ui(13)).foregroundStyle(palette.tertiary).lineLimit(1)
                    .frame(maxWidth: .infinity, alignment: .leading)
                if let aside = row.aside { Text(aside).font(Theme.ui(12)).foregroundStyle(palette.inkFaint) }
                if let state = row.state { AgentStateMark(group: state) }
            }
            .padding(.horizontal, 12)
            .frame(minHeight: 44)
            .contentShape(Rectangle())
        }
        .buttonStyle(ActiveFill(radius: 9))
        .padding(.horizontal, 3)
        .accessibilityLabel([row.title, row.detail].compactMap { $0 }.joined(separator: ", "))
    }
}
