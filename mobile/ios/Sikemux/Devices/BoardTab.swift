import SikemuxCore
import SwiftUI

private let columns: [BoardColumn] = [.needs, .progress, .open, .done]

private func columnLabel(_ column: BoardColumn) -> String {
    switch column {
    case .open: "Open"
    case .progress: "In progress"
    case .needs: "Needs you"
    case .done: "Done"
    case .unknown: "Other"
    }
}

/// The first words of a title, for a chip that points at the card.
private func shortTitle(_ title: String) -> String {
    let words = title.split(whereSeparator: \.isWhitespace)
    return words.count > 3 ? words.prefix(3).joined(separator: " ") + "…" : title
}

/// The cards a board shows: those put aside in Later stay off it, as on the Mac.
private func onBoard(_ cards: [BoardCard]) -> [BoardCard] {
    cards.filter { $0.asideAt == nil }
}

/// A host call read while the board shows, and again whenever `seen` changes.
@MainActor @Observable
private final class Reading<Answer> {
    var data: Answer?
    var problem: String?

    func read(_ live: HostLive, _ ask: (Connection) async throws -> Answer) async {
        guard let connection = live.connection else { return }
        do {
            data = try await ask(connection)
            problem = nil
        } catch {
            problem = Sikemux.problem(error)
        }
    }
}

/// A project's board, read only: cards move on the Mac or by the agents themselves. Unscoped, a row per board.
struct BoardTab: View {
    let core: String
    let live: HostLive
    let view: DeviceView
    let scope: ProjectInfo?
    let onScope: (String?) -> Void
    @State private var questions = Reading<[AgentQuestion]>()

    var body: some View {
        let seen = Self.seen(view)
        VStack(spacing: 0) {
            if let problem = questions.problem { BoardProblem(what: "the agents' questions", text: problem) }
            if let scope {
                OneBoard(core: core, live: live, view: view, scope: scope, questions: questions.data ?? [], seen: seen)
            } else {
                AllBoards(live: live, view: view, seen: seen, onScope: onScope)
            }
        }
        .task(id: seen) { await questions.read(live) { try await $0.call(PhoneCall.Questions.self, NoParams()) } }
    }

    /// Something on the host changed that may have moved cards: an agent started, finished or asked.
    private static func seen(_ view: DeviceView) -> String {
        let chats = view.chats.map { "\($0.agentId):\($0.group?.rawValue ?? ""):\($0.running):\($0.activeAt ?? 0)" }
        let terminals = view.sessions.map { "\($0.agentId ?? ""):\($0.agentState.map { "\($0)" } ?? "")" }
        return (chats + terminals).joined(separator: " ")
    }
}

private struct BoardProblem: View {
    let what: String
    let text: String

    var body: some View {
        EmptyState(icon: Icons.warning, text: "Couldn't read \(what): \(text)")
    }
}

private struct OneBoard: View {
    @Environment(\.palette) private var palette
    let core: String
    let live: HostLive
    let view: DeviceView
    let scope: ProjectInfo
    let questions: [AgentQuestion]
    let seen: String
    @State private var board = Reading<ProjectBoard>()

    var body: some View {
        VStack(spacing: 0) {
            if let problem = board.problem {
                BoardProblem(what: "the board", text: problem)
            } else if let read = board.data {
                let cards = onBoard(read.cards).sorted { $0.updatedAt > $1.updatedAt }
                let byId = Dictionary(cards.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
                if !read.triggers.isEmpty { Triggers(triggers: read.triggers) }
                if cards.isEmpty && read.triggers.isEmpty {
                    EmptyState(icon: Icons.board, text: "No cards on \(scope.name)'s board.")
                }
                ForEach(columns, id: \.self) { column in
                    let shown = cards.filter { $0.column == column }
                    if !shown.isEmpty {
                        SectionLabel(columnLabel(column), count: shown.count)
                        Rows {
                            ForEach(shown, id: \.id) { card in
                                CardRow(core: core, card: card, cards: cards, byId: byId, view: view, questions: questions)
                            }
                        }
                    }
                }
                BetweenAgents(live: live, view: view, project: scope.path)
            }
        }
        .task(id: "\(scope.path) \(seen)") {
            await board.read(live) { try await $0.call(PhoneCall.Board.self, BoardParams(project: scope.path)) }
        }
    }
}

/// The triggers that add a card to this board each time they fire.
private struct Triggers: View {
    @Environment(\.palette) private var palette
    let triggers: [ChatTrigger]
    private static let weekdays = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"]

    var body: some View {
        SectionLabel("Triggers", count: triggers.count)
        Rows {
            ForEach(triggers, id: \.id) { trigger in
                Row(title: trigger.title, detail: Self.when(trigger),
                    mark: { IconView(icon: Icons.timer, size: 18).foregroundStyle(palette.cmd) }, end: { EmptyView() })
            }
        }
    }

    /// How often a trigger fires and when next, as the Mac's board says it.
    static func when(_ trigger: ChatTrigger) -> String {
        if case .ci(let ci) = trigger.when { return "when CI on \(ci.on) ends" }
        guard let nextAt = trigger.nextAt else { return "on each request" }
        if case .webhook = trigger.when { return "on each request" }
        let at = Format.date(millis: nextAt)
        let time = at.formatted(date: .omitted, time: .shortened)
        let weekday = Calendar.current.component(.weekday, from: at) - 1
        let next = Calendar.current.isDateInToday(at) ? "today \(time)" : "\(weekdays[weekday]) \(time)"
        switch trigger.when {
        case .every(let every):
            let minutes = Int((Double(every.seconds) / 60).rounded(.up))
            let span = minutes % 1440 == 0 ? "\(minutes / 1440) d" : minutes % 60 == 0 ? "\(minutes / 60) h" : "\(minutes) min"
            return "every \(span) · next \(next)"
        case .daily(let daily):
            let days = daily.weekdays == [1, 2, 3, 4, 5] ? "weekdays"
                : daily.weekdays.isEmpty ? "daily" : daily.weekdays.map { weekdays[$0] }.joined(separator: " ")
            return "\(days) · next \(next)"
        default:
            return next
        }
    }
}

/// Who messaged whom among the project's agents, read again whenever the host logs another.
private struct BetweenAgents: View {
    @Environment(\.palette) private var palette
    let live: HostLive
    let view: DeviceView
    let project: String
    @State private var messages = Reading<AgentMessageRows>()
    private static let shownMost = 20
    private static let senders = ["person": "You", "board": "Agent board", "auto": "Auto", "sikemux": "Sikemux"]

    var body: some View {
        let logged = messages.data?.project ?? project
        let latest = view.inbox.agentMessages[logged] ?? 0
        VStack(spacing: 0) {
            if let problem = messages.problem {
                BoardProblem(what: "the messages between agents", text: problem)
            } else if let rows = messages.data?.rows.prefix(Self.shownMost), !rows.isEmpty {
                SectionLabel("Between agents")
                Rows {
                    ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
                        let cli = row.from.flatMap(cliOf)
                        Row(
                            title: "\(Self.who(row.from, row.fromName)) to \(Self.who(row.to, row.toName))",
                            detailText: Text("\(Self.kind(row.kind)) · ") + Text(Self.outcome(row)).foregroundColor(
                                row.tone == .held ? palette.warn : row.tone == .failed ? palette.danger : palette.tertiary),
                            mark: {
                                if let cli { AgentMark(provider: cli, size: 20) } else { Dot(color: palette.rest, size: 9, hollow: true) }
                            },
                            end: {
                                Text(Format.age(Format.date(millis: row.sentAt)))
                                    .metaStyle(palette)
                                    .monospacedDigit()
                            })
                    }
                }
            }
        }
        .task(id: "\(project) \(latest)") {
            await messages.read(live) { try await $0.call(PhoneCall.AgentMessageRows.self, AgentMessagesParams(project: project)) }
        }
    }

    private func cliOf(_ id: String) -> String? {
        view.chats.first { $0.agentId == id }?.provider ?? view.sessions.first { $0.agentId == id }?.agentType
    }

    /// Who sent or got a message: a sender that is not an agent by what it is, an agent by its name.
    private static func who(_ id: String?, _ name: String?) -> String {
        guard let id else { return "An agent" }
        return senders[id] ?? name.map { "@\($0)" } ?? "A closed agent"
    }

    private static func kind(_ kind: MessageKind?) -> String {
        switch kind {
        case .report: "Report"
        case .question: "Question"
        case .answer: "Answer"
        case .notice: "Note"
        default: "Message"
        }
    }

    private static func outcome(_ row: MessageRow) -> String {
        switch row.outcome {
        case .steered: "Read mid-turn"
        case .newTurn: "Started a turn"
        case .typed: "Typed in"
        case .waiting: "Waiting"
        case .delivered: row.waited ? "Delivered after waiting" : "Delivered"
        case .returned: "Not taken, waiting"
        case .repeat: "Repeat, not sent"
        case .refused: "Refused"
        case .dropped: "Dropped unread"
        case .unknown: "Sent"
        }
    }
}

private struct CardRow: View {
    @Environment(\.palette) private var palette
    let core: String
    let card: BoardCard
    let cards: [BoardCard]
    let byId: [Int: BoardCard]
    let view: DeviceView
    let questions: [AgentQuestion]

    var body: some View {
        let chat = card.owner.flatMap { owner in view.chats.first { $0.agentId == owner } }
        let terminal = card.owner.flatMap { owner in view.sessions.first { $0.agentId == owner && $0.running } }
        let owner = chat?.name ?? terminal?.agentName ?? card.ownerName
        let cli = chat?.provider ?? terminal?.agentType ?? card.ownerCli
        let group = view.inbox.agents.first { $0.agentId == card.owner }?.group
        let question = questions.first { $0.id == card.question }
        let handedBack = card.auto?.waitsOnPerson == true && card.status != .done ? card.auto?.note : nil
        let reviewing = card.auto?.stage == .reviewing ? card.auto?.note : nil
        let acceptance = !card.acceptance.isEmpty && card.status != .done ? "Done when: \(card.acceptance)" : nil
        let detail: Text? = question.map {
            Text("@\($0.name)").font(Theme.ui(12, .medium)).foregroundColor(palette.secondary) + Text(" asks: \($0.text)")
        } ?? (handedBack ?? reviewing ?? acceptance).map { Text($0) }

        Row(
            title: card.title, detailText: detail, underText: under(owner: owner), dim: card.status == .done, detailLines: 2,
            onPress: chat.map { chat in { Router.shared.push(.chat(core: core, agent: chat.agentId)) } },
            mark: {
                if let cli { AgentMark(provider: cli, size: 20) } else { Dot(color: palette.rest, size: 9, hollow: true) }
            },
            end: {
                if card.status == .done {
                    IconView(icon: Icons.check, size: 13).foregroundStyle(palette.tertiary)
                } else if card.column == .needs || group == .blocked {
                    NeedsYou()
                } else if group == .working {
                    Working()
                } else if group == .done {
                    Dot(color: palette.accent)
                }
            })
    }

    private func under(owner: String?) -> Text {
        let key = { (text: String) in Text(text).font(Theme.mono(11)) }
        let children = cards.filter { $0.parent == card.id }
        let done = children.filter { $0.status == .done }.count
        let waits = card.waitsOn.compactMap { byId[$0] }
        let after = waits.first ?? card.dependsOn.first.flatMap { byId[$0] }
        let parent = card.parent.flatMap { byId[$0] }
        var parts: [Text] = [owner.map { Text("@\($0)").font(Theme.ui(12, .medium)).foregroundColor(palette.secondary) } ?? Text("Nobody yet")]
        if let ticket = card.ticket {
            parts.append(key(ticket.key) + Text(ticket.source.map { " \($0)" } ?? ""))
        } else if let filed = card.ticketAbove {
            parts.append(Text("under ") + key(filed))
        } else if let parent {
            parts.append(Text("under \(shortTitle(parent.title))"))
        }
        if !children.isEmpty { parts.append(key("\(done)/\(children.count)")) }
        if let after { parts.append(Text("after \(shortTitle(after.title))")) }
        return parts.dropFirst().reduce(parts[0]) { $0 + Text(" · ") + $1 }
    }
}

private struct AllBoards: View {
    @Environment(\.palette) private var palette
    let live: HostLive
    let view: DeviceView
    let seen: String
    let onScope: (String?) -> Void
    @State private var boards = Reading<[ProjectBoard]>()

    var body: some View {
        VStack(spacing: 0) {
            if let problem = boards.problem {
                BoardProblem(what: "the boards", text: problem)
            } else if let read = boards.data {
                let shown = read.map { ($0.project, onBoard($0.cards)) }.filter { !$0.1.isEmpty }
                if shown.isEmpty {
                    EmptyState(icon: Icons.board, text: "No board has cards yet.")
                } else {
                    SectionLabel("Boards")
                    Rows {
                        ForEach(shown, id: \.0) { place, cards in
                            let project = view.project(at: place)
                            let counts = columns.compactMap { column -> String? in
                                let count = cards.filter { $0.column == column }.count
                                return count > 0 ? "\(count) \(columnLabel(column).lowercased())" : nil
                            }
                            Row(
                                title: view.projectName(place), detail: counts.joined(separator: " · "),
                                onPress: project.map { project in { onScope(project.id) } },
                                mark: { IconView(icon: Icons.folder, size: 18).foregroundStyle(palette.live) },
                                end: { if cards.contains(where: { $0.column == .needs }) { NeedsYou() } })
                        }
                    }
                }
            }
        }
        .task(id: seen) { await boards.read(live) { try await $0.call(PhoneCall.Boards.self, NoParams()) } }
    }
}
