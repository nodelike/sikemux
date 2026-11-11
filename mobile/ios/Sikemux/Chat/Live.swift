import SwiftUI

/// What the agent still has going besides the turn, and what waits to be sent.
struct LiveWork: Equatable {
    var subagents: [SubagentCard]
    var tasks: [(kind: String, tasks: [AsyncTask])]
    var queued: [QueuedMessage]
    var plan: [PlanEntry]

    init(_ rows: ChatRows) {
        subagents = rows.head.runningSubagents
        var kinds: [String] = []
        var byKind: [String: [AsyncTask]] = [:]
        for task in rows.head.tasks {
            let kind = task.taskType.isEmpty ? "task" : task.taskType
            if byKind[kind] == nil { kinds.append(kind) }
            byKind[kind, default: []].append(task)
        }
        tasks = kinds.map { ($0, byKind[$0]!) }
        queued = rows.queued
        plan = rows.head.plan ?? []
    }

    static func == (a: LiveWork, b: LiveWork) -> Bool {
        a.subagents == b.subagents && a.queued == b.queued && a.plan == b.plan
            && a.tasks.map(\.kind) == b.tasks.map(\.kind) && a.tasks.map(\.tasks) == b.tasks.map(\.tasks)
    }

    /// Some step of the plan is still to do.
    var planOpen: Bool { plan.contains { $0.status != .completed } }
    var any: Bool { !subagents.isEmpty || !tasks.isEmpty || !queued.isEmpty || planOpen }

    struct Piece: Equatable {
        var key: String
        var task: String?
        var count: String
        var word: String
    }

    var pieces: [Piece] {
        var pieces: [Piece] = []
        if !subagents.isEmpty {
            pieces.append(
                Piece(key: "subagents", count: "\(subagents.count)", word: Format.counted("subagent", subagents.count)))
        }
        for (kind, tasks) in tasks {
            pieces.append(
                Piece(key: "task-\(kind)", task: kind, count: "\(tasks.count)", word: Format.counted(kind, tasks.count))
            )
        }
        if !queued.isEmpty { pieces.append(Piece(key: "queued", count: "\(queued.count)", word: "queued")) }
        if planOpen {
            let done = plan.filter { $0.status == .completed }.count
            pieces.append(Piece(key: "plan", count: "\(done)/\(plan.count)", word: "plan"))
        }
        return pieces
    }

    private var running: [AsyncTask] { tasks.flatMap { $0.tasks.filter { $0.state == .running } } }

    /// The running things the strip turns through.
    var lines: [String] {
        subagents.map { "\($0.name) · \(Self.activity($0))" }
            + running.flatMap { task in
                [[task.name, Self.detail(task)].compactMap { $0 }.joined(separator: " · ")]
                    + (task.agents ?? []).filter { $0.state == .running }.map {
                        [$0.label, Self.agentDetail($0)].compactMap { $0 }.joined(separator: " · ")
                    }
            }
    }

    var working: Bool { !subagents.isEmpty || !running.isEmpty }

    /// What a subagent is up to: the call it made last says more than the prose it wrote before starting.
    static func activity(_ subagent: SubagentCard) -> String {
        if let tool = subagent.lastTool {
            return "\(ChatWords.toolKind(tool)) \(tool.row.target)".trimmingCharacters(in: .whitespaces)
        }
        return subagent.task.components(separatedBy: "\n").first?.trimmingCharacters(in: .whitespaces) ?? ""
    }

    /// A task whose description repeats its name would print the same words twice, so the detail takes the first
    /// thing that says more.
    static func detail(_ task: AsyncTask) -> String? {
        let detail = [task.summary, task.description, task.lastToolName, task.taskType].compactMap { $0 }.first {
            !$0.isEmpty && $0 != task.name
        }
        let joined = [task.owner.map { "from \($0)" }, detail].compactMap { $0 }.joined(separator: " · ")
        return joined.isEmpty ? nil : joined
    }

    static func agentDetail(_ agent: WorkflowAgent) -> String? {
        if agent.state == .failed { return agent.error ?? agent.phase }
        if agent.state == .running, let activity = agent.activity { return activity }
        return agent.phase
    }

    static func agentCost(_ agent: WorkflowAgent) -> String? {
        let cost = [agent.tokens.map { "\(tokenCount($0)) tokens" }, agent.toolCalls.map { "\($0) tools" }].compactMap {
            $0
        }
        return cost.isEmpty ? nil : cost.joined(separator: " · ")
    }

    static func agentCount(_ agents: [WorkflowAgent]) -> String {
        let finished = agents.filter { $0.state == .completed || $0.state == .failed }.count
        return "\(finished)/\(agents.count) \(agents.count == 1 ? "agent" : "agents")"
    }

    static func queuedLabel(_ message: QueuedMessage) -> String {
        message.text.isEmpty
            ? (message.paths.map { ($0 as NSString).lastPathComponent } + message.context.map(\.title)).joined(
                separator: ", ") : message.text
    }
}

private func taskIcon(_ kind: String) -> IconDef {
    switch kind {
    case "shell": Icons.named("IconCommand") ?? Icons.terminal
    case "workflow": Icons.workflow
    default: Icons.timer
    }
}

/// One line docked on the composer saying what is still going: subagents, background tasks by kind, waiting messages
/// and the plan's progress. A tap opens all of it.
struct LiveStrip: View {
    @Environment(\.palette) private var palette
    let work: LiveWork
    let provider: String
    let onOpen: () -> Void
    @State private var turn = 0

    /// Past this many kinds the strip drops the words and keeps the counts.
    private static let worded = 3

    var body: some View {
        let pieces = work.pieces
        let lines = work.lines
        let worded = lines.isEmpty && pieces.count <= Self.worded
        Button(action: onOpen) {
            HStack(spacing: 10) {
                if work.working { Working() }
                if !lines.isEmpty {
                    Shimmer(text: lines[turn % lines.count], font: Theme.ui(12.5), color: palette.secondary)
                        .id(turn % lines.count)
                        .transition(.opacity)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                HStack(spacing: lines.isEmpty ? 12 : 10) {
                    ForEach(pieces, id: \.key) { piece in
                        HStack(spacing: 5) {
                            pieceIcon(piece)
                            (Text(piece.key == "plan" && worded ? "\(piece.word) " : "")
                                + Text(piece.count).font(Theme.ui(12, .medium)).foregroundColor(palette.ink)
                                + Text(piece.key != "plan" && worded ? " \(piece.word)" : ""))
                                .font(Theme.ui(12)).foregroundStyle(palette.tertiary).monospacedDigit().lineLimit(1)
                        }
                    }
                }
                .frame(maxWidth: lines.isEmpty ? .infinity : nil, alignment: .leading)
                IconView(icon: Icons.chevron, size: 10).foregroundStyle(palette.inkFaint).rotationEffect(.degrees(-90))
            }
            .padding(.leading, 12)
            .padding(.trailing, 10)
            .frame(height: 34)
            .background(UnevenRoundedRectangle(topLeadingRadius: 12, topTrailingRadius: 12).fill(palette.composer))
            .overlay(
                UnevenRoundedRectangle(topLeadingRadius: 12, topTrailingRadius: 12).stroke(palette.border, lineWidth: 1)
                    .padding(.bottom, -2).clipped())
        }
        .buttonStyle(.plain)
        .padding(.horizontal, 14)
        .padding(.bottom, -1)
        .accessibilityLabel(pieces.map { "\($0.count) \($0.word)" }.joined(separator: ", ") + ". Show")
        .task(id: lines.count) {
            guard lines.count > 1 else { return }
            while !Task.isCancelled {
                try? await Task.sleep(for: .milliseconds(3200))
                withAnimation(.easeInOut(duration: 0.14)) { turn += 1 }
            }
        }
    }

    @ViewBuilder private func pieceIcon(_ piece: LiveWork.Piece) -> some View {
        if let task = piece.task {
            IconView(icon: taskIcon(task), size: 12).foregroundStyle(palette.inkDim)
        } else if piece.key == "subagents" {
            AgentMark(provider: provider, size: 12)
        } else {
            IconView(icon: piece.key == "queued" ? Icons.clock : Icons.check, size: 12).foregroundStyle(palette.inkDim)
        }
    }
}

/// Everything the strip sums up, in the Mac's live stack's order, with what can be done about each.
struct LiveSheet: View {
    @Environment(\.palette) private var palette
    let work: LiveWork
    let provider: String
    /// The agent takes a message mid-turn, and a turn is running.
    let steerable: Bool
    /// This phone may act on the chat rather than only watch it.
    let canAct: Bool
    let session: ChatSession
    let onSteered: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            if !work.subagents.isEmpty {
                SectionLabel(Format.counted("subagent", work.subagents.count), count: work.subagents.count)
                ForEach(work.subagents, id: \.sessionId) { subagent in
                    Line(title: subagent.name, detail: LiveWork.activity(subagent), cost: SubagentRow.cost(subagent)) {
                        AgentMark(provider: provider, size: 16)
                    } end: {
                        Working()
                    }
                }
            }
            ForEach(work.tasks, id: \.kind) { kind, tasks in
                SectionLabel(Format.counted(kind, tasks.count), count: tasks.count)
                ForEach(tasks, id: \.asyncTaskId) { task in
                    Line(
                        title: task.name, detail: task.state == .paused ? "paused" : LiveWork.detail(task),
                        cost: task.agents.map(LiveWork.agentCount)
                    ) {
                        IconView(icon: taskIcon(kind), size: 15).foregroundStyle(
                            task.state == .paused ? palette.warn : palette.inkDim)
                    } end: {
                        if canAct && task.canStop {
                            SmallButton(label: "Stop", busy: session.stopping.contains(task.asyncTaskId)) {
                                session.stopTask(task.asyncTaskId)
                            }
                            .accessibilityLabel("Stop \(task.name)")
                        }
                    }
                    ForEach(task.agents ?? [], id: \.index) { agent in
                        Line(
                            title: agent.label, detail: LiveWork.agentDetail(agent), cost: LiveWork.agentCost(agent),
                            faint: agent.state == .queued || agent.state == .completed, nested: true
                        ) {
                            AgentMark(provider: provider, size: 16)
                        } end: {
                            switch agent.state {
                            case .running: Working()
                            case .completed:
                                IconView(icon: Icons.check, size: 12).foregroundStyle(
                                    palette.live.mixed(with: palette.inkFaint, 0.58))
                            case .failed: IconView(icon: Icons.close, size: 11).foregroundStyle(palette.danger)
                            default: EmptyView()
                            }
                        }
                    }
                }
            }
            if !work.queued.isEmpty {
                SectionLabel(text: "queued", count: work.queued.count) {
                    if steerable && work.queued.count > 1 {
                        SmallButton(label: "Steer all") { steer(work.queued.map(\.id)) }
                            .accessibilityLabel("Steer the running turn with every queued message")
                    }
                }
                ForEach(work.queued, id: \.id) { held in
                    let label = LiveWork.queuedLabel(held)
                    let files = held.paths.count
                    Line(
                        title: label,
                        detail: !held.text.isEmpty && files > 0 ? "\(files) \(Format.counted("file", files))" : nil
                    ) {
                        IconView(icon: Icons.clock, size: 15).foregroundStyle(palette.inkDim)
                    } end: {
                        if steerable {
                            SmallButton(label: "Steer") { steer([held.id]) }.accessibilityLabel(
                                "Steer the running turn with \(label)")
                        }
                        Button {
                            session.dropQueued(held.id)
                        } label: {
                            IconView(icon: Icons.close, size: 11).foregroundStyle(palette.inkDim).frame(
                                width: 28, height: 28)
                        }
                        .buttonStyle(ActiveFill(radius: 8))
                        .accessibilityLabel("Drop \(label) from the queue")
                    }
                }
            }
            if !work.plan.isEmpty {
                let done = work.plan.filter { $0.status == .completed }.count
                SectionLabel(text: "plan") {
                    Text("\(done) of \(work.plan.count)").font(Theme.ui(11)).monospacedDigit().foregroundStyle(
                        palette.inkDim)
                }
                ForEach(Array(work.plan.enumerated()), id: \.offset) { _, entry in
                    HStack(alignment: .top, spacing: 10) {
                        Group {
                            switch entry.status {
                            case .completed: IconView(icon: Icons.check, size: 14).foregroundStyle(palette.inkDim)
                            case .inProgress: Working()
                            default: Circle().strokeBorder(palette.rest, lineWidth: 1.5).frame(width: 8, height: 8)
                            }
                        }
                        .frame(width: 20, height: 20)
                        Text(entry.content)
                            .font(Theme.ui(14)).lineSpacing(3)
                            .foregroundStyle(
                                entry.status == .completed
                                    ? palette.tertiary : entry.status == .inProgress ? palette.ink : palette.secondary
                            )
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    .padding(.horizontal, 8)
                    .padding(.vertical, 7)
                }
            }
        }
        .padding(.bottom, 8)
    }

    private func steer(_ ids: [String]) {
        session.steer(ids)
        onSteered()
    }
}

private struct Line<Mark: View, End: View>: View {
    @Environment(\.palette) private var palette
    let title: String
    var detail: String?
    /// What a subagent has spent so far, or how many of a workflow's agents are done.
    var cost: String?
    var faint = false
    /// One of a workflow's agents, set in under the workflow.
    var nested = false
    @ViewBuilder let mark: Mark
    @ViewBuilder let end: End

    var body: some View {
        HStack(spacing: 10) {
            mark.frame(width: 20, height: 20).opacity(faint ? 0.6 : 1)
            VStack(alignment: .leading, spacing: 1) {
                Text(title).font(Theme.ui(14.5, .medium)).tracking(-0.15).foregroundStyle(
                    faint ? palette.tertiary : palette.secondary
                ).lineLimit(1)
                if detail != nil || cost != nil {
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        if let detail { Text(detail).font(Theme.ui(12)).foregroundStyle(palette.tertiary).lineLimit(1) }
                        if let cost {
                            Text(cost).font(Theme.mono(10.5)).monospacedDigit().foregroundStyle(palette.inkFaint)
                        }
                    }
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            HStack(spacing: 8) { end }
        }
        .frame(minHeight: 48)
        .padding(.horizontal, 8)
        .padding(.vertical, 6)
        .padding(.leading, nested ? 30 : 0)
    }
}

private struct SmallButton: View {
    @Environment(\.palette) private var palette
    let label: String
    var busy = false
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Group {
                if busy {
                    ProgressView().controlSize(.small)
                } else {
                    Text(label).font(Theme.ui(12.5, .medium)).foregroundStyle(palette.ink)
                }
            }
            .frame(minWidth: 52, minHeight: 28)
            .padding(.horizontal, 10)
            .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(palette.borderStrong, lineWidth: 1))
        }
        .buttonStyle(ActiveFill(radius: 8))
        .disabled(busy)
    }
}
