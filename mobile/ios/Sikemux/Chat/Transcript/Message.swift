import SwiftUI

/// What a strip under a message shows: when it was sent or its turn finished, how long that took, how fast it was
/// written, and what Copy gives.
struct Strip: Equatable {
    var at: Int?
    var took: Int?
    var rate: Double?
    var text: String

    /// The message a strip sits under: a prompt itself; an answer, which the agent splits into a message per tool
    /// call, the last of its turn, once that turn has finished.
    static func owner(_ messages: [ChatMessage], _ id: String) -> String? {
        guard var at = messages.firstIndex(where: { $0.id == id }) else { return nil }
        if messages[at].role == .user { return id }
        let start = at
        while at < messages.count, messages[at].role == .assistant {
            if at > start, messages[at].opensTurn != nil { return nil }
            if messages[at].meta != nil { return messages[at].id }
            at += 1
        }
        return nil
    }

    static func of(_ messages: [ChatMessage], _ id: String) -> Strip? {
        guard let index = messages.firstIndex(where: { $0.id == id }) else { return nil }
        let message = messages[index]
        let text = copyText(messages, index)
        if message.role == .user { return Strip(at: message.sentAt, text: text) }
        guard let meta = message.meta else { return nil }
        return Strip(at: meta.at, took: meta.took, rate: meta.rate, text: text)
    }

    /// What a reader means by "copy this message": the prose, the agent's reasoning where it showed it, and the files
    /// a turn was sent with.
    private static func messageText(_ message: ChatMessage) -> String {
        var blocks: [String] = message.parts.compactMap { part in
            switch part {
            case .text(let text): text.text.trimmingCharacters(in: .whitespacesAndNewlines)
            case .thought(let thought): thought.text.trimmingCharacters(in: .whitespacesAndNewlines)
            default: nil
            }
        }.filter { !$0.isEmpty }
        blocks += message.attachments ?? []
        return blocks.joined(separator: "\n\n")
    }

    private static func copyText(_ messages: [ChatMessage], _ index: Int) -> String {
        let message = messages[index]
        if message.role == .user { return messageText(message) }
        guard let meta = message.meta else { return "" }
        let from = messages.firstIndex { $0.id == meta.from } ?? index
        return messages[min(from, index)...index].map(messageText).filter { !$0.isEmpty }.joined(separator: "\n\n")
    }
}

/// One row of the transcript: what goes above it, then the person's message or the agent's reply.
struct TranscriptRow: View {
    @Environment(\.palette) private var palette
    @Environment(TranscriptContext.self) private var context
    let entry: RowEntry

    var body: some View {
        let look = entry.look
        VStack(alignment: .leading, spacing: 0) {
            if let worktree = look.worktree { WorktreeLine(worktree: worktree) }
            if let line = look.team, let id = look.teamId {
                TeamFoldLine(
                    line: line, open: look.teamOpen, all: look.teamAll, onToggle: { context.actions.toggleRun(id) },
                    onAll: { context.actions.showRunEarlier(id) })
            }
            if let fold = look.summary {
                WorkSummary(took: fold.took, calls: fold.calls, thought: fold.thought, open: look.workOpen) {
                    context.actions.toggleWork(fold.id)
                }
            }
            if !look.hidden { TranscriptMessage(message: entry.message, look: look) }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

struct TranscriptMessage: View {
    @Environment(\.palette) private var palette
    @Environment(TranscriptContext.self) private var context
    let message: ChatMessage
    var look = RowLook()

    var body: some View {
        if let letters = message.letters, !letters.isEmpty {
            Letters(message: message, letters: letters, strip: look.strip)
                .contentShape(Rectangle())
                .onTapGesture { tap() }
        } else if message.role == .user {
            UserMessage(message: message, look: look, tap: tap)
        } else {
            VStack(alignment: .leading, spacing: 0) {
                Parts(id: message.id, parts: shownParts, live: look.live)
                if let strip = look.strip { StripView(strip: strip, mine: false) }
            }
            .contentShape(Rectangle())
            .onTapGesture { tap() }
            .accessibilityHint("Tap for copy and how long it took")
        }
    }

    private var shownParts: [ChatPart] {
        guard let kept = look.kept else { return message.parts }
        return message.parts.enumerated().filter { kept.contains($0.offset) }.map(\.element)
    }

    private func tap() {
        Haptics.select()
        context.actions.tap(message)
    }
}

private struct UserMessage: View {
    @Environment(\.palette) private var palette
    @Environment(TranscriptContext.self) private var context
    let message: ChatMessage
    let look: RowLook
    let tap: () -> Void

    var body: some View {
        let failed = look.unsent?.state == .failed
        let typed = message.parts.compactMap { part -> ChatPart.Text? in
            if case .text(let text) = part { return text }
            return nil
        }
        let text = typed.map(\.text).joined(separator: "\n\n")
        let spans = message.spans ?? []
        let context = (message.context ?? []).filter { item in !spans.contains { $0.chip?.carried == item.uri } }
        let attachments = message.attachments ?? []
        VStack(alignment: .trailing, spacing: 0) {
            if !attachments.isEmpty { SentAttachments(paths: attachments) }
            if !context.isEmpty { ContextChips(items: context) }
            if look.editing {
                MessageEditor(rowId: message.id, text: text)
            } else if !text.isEmpty || (attachments.isEmpty && context.isEmpty) {
                Text(words(text: text, spans: spans, typed: typed))
                    .sentWords(palette)
                    .textSelection(.enabled)
                    .mineBubble(palette, failed: failed)
                    .opacity(look.unsent?.state == .sending ? 0.7 : 1)
                    .fixedSize(horizontal: false, vertical: true)
                    .sentWidth()
                    .onTapGesture { tap() }
                    .accessibilityHint("Tap for copy and the time it was sent")
            }
            if failed {
                Button {
                    self.context.actions.retry(message.id)
                } label: {
                    HStack(spacing: 5) {
                        IconView(icon: Icons.warning, size: 12).foregroundStyle(palette.danger)
                        Text("Not sent").foregroundStyle(palette.danger)
                        Text("·").foregroundStyle(palette.inkFaint)
                        Text("Retry").foregroundStyle(palette.ink)
                    }
                    .font(Theme.ui(12, .medium))
                    .padding(.top, 5)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Not sent\(look.unsent?.problem.map { ": \($0)" } ?? ""). Retry")
            } else if look.unsent?.state == .sending {
                Text("Sending…").font(Theme.ui(11)).foregroundStyle(palette.inkFaint).padding(.top, 4)
            } else if let strip = look.strip, !look.editing {
                StripView(
                    strip: strip, mine: true, onEdit: look.editable ? { self.context.actions.edit(message) } : nil)
            } else if !look.followedByOwn, !look.editing, let sentAt = message.sentAt {
                Text(ChatWords.clock(sentAt)).font(Theme.ui(11)).monospacedDigit().foregroundStyle(palette.inkFaint)
                    .frame(height: 24)
            }
        }
        .frame(maxWidth: .infinity, alignment: .trailing)
        .padding(.top, look.followsOwn ? 4 : 20)
        .padding(.bottom, look.followedByOwn ? 0 : 10)
    }

    private func words(text: String, spans: [SentSpan], typed: [ChatPart.Text]) -> AttributedString {
        if let finding = context.finding { return finding.marked(typed, palette: palette) }
        guard !spans.isEmpty else { return AttributedString(text) }
        var out = AttributedString()
        for span in spans {
            if let chip = span.chip {
                var run = AttributedString(chip.label)
                run.foregroundColor = palette.accent
                run.backgroundColor = palette.active
                out += run
            } else {
                out += AttributedString(span.text)
            }
        }
        return out
    }
}

/// What the Mac shows under a message on hover: Copy, Edit on a message that can be written again, when it was sent
/// or finished and how long it took, and how fast it was written.
private struct StripView: View {
    @Environment(\.palette) private var palette
    let strip: Strip
    let mine: Bool
    var onEdit: (() -> Void)?

    var body: some View {
        HStack(spacing: 8) {
            if mine { Spacer(minLength: 0) }
            CopyButton(value: strip.text, label: mine ? "message" : "reply", size: 15)
                .frame(width: 24, height: 24)
            if let onEdit {
                Button(action: onEdit) {
                    IconView(icon: Icons.pencil, size: 15).foregroundStyle(palette.inkFaint).frame(
                        width: 24, height: 24)
                }
                .buttonStyle(ActiveFill(radius: 7))
                .accessibilityLabel("Edit message")
            }
            if let at = strip.at {
                Text(
                    strip.took.map { "\(ChatWords.sent(at)) · took \(ChatWords.duration(ms: Double($0)))" }
                        ?? ChatWords.sent(at)
                )
                .font(Theme.ui(11))
                .monospacedDigit()
                .foregroundStyle(palette.inkDim)
            }
            if let rate = strip.rate {
                Spacer(minLength: 0)
                Text(ChatWords.rate(rate)).font(Theme.mono(11)).italic().foregroundStyle(palette.inkDim)
            }
        }
        .frame(height: 24)
        .padding(.top, 6)
    }
}

/// A message made of messages from other agents and answers.
private struct Letters: View {
    @Environment(\.palette) private var palette
    @Environment(TranscriptContext.self) private var context
    let message: ChatMessage
    let letters: [AgentLetter]
    let strip: Strip?

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            ForEach(LetterGroup.of(letters), id: \.at) { group in
                switch group.kind {
                case .answers(let answers):
                    let first = answers[0]
                    AnsweredQuestions(
                        agentId: first.agentId ?? "", cli: first.cli,
                        asker: first.agentId == context.agentId ? nil : first.name,
                        answers: answers.map {
                            Answered(
                                question: $0.question ?? "", answer: $0.text, secret: $0.secret ?? false,
                                files: $0.attachments ?? [])
                        })
                case .letter(let letter):
                    if let trigger = letter.trigger {
                        FiredTrigger(id: "\(message.id)/letter/\(group.at)", fired: trigger, at: message.sentAt)
                    } else {
                        From(at: message.sentAt) {
                            AgentFaceView(agentId: letter.agentId, provider: letter.cli, size: 17)
                            Text("@\(letter.name)").font(Theme.ui(12.5, .semibold)).foregroundStyle(palette.ink)
                                .lineLimit(1)
                        } content: {
                            LongText(
                                id: "\(message.id)/letter/\(group.at)", text: letter.text, md: letter.tree, live: false,
                                style: ProseStyle(size: 14, lineHeight: 22, color: palette.ink))
                        }
                    }
                }
            }
            if let strip {
                StripView(
                    strip: Strip(
                        at: strip.at, took: strip.took, rate: strip.rate,
                        text: letters.map(\.text).joined(separator: "\n\n")), mine: false)
            }
        }
        .padding(.top, 14)
        .padding(.bottom, 6)
    }
}

/// A message's letters, with each run of answers to one agent's questions together as one thread.
private struct LetterGroup {
    enum Kind {
        case answers([AgentLetter])
        case letter(AgentLetter)
    }

    let at: Int
    var kind: Kind

    static func of(_ letters: [AgentLetter]) -> [LetterGroup] {
        var groups: [LetterGroup] = []
        for (at, letter) in letters.enumerated() {
            if letter.kind != .answer {
                groups.append(LetterGroup(at: at, kind: .letter(letter)))
            } else if case .answers(var answers) = groups.last?.kind, answers[0].agentId == letter.agentId {
                answers.append(letter)
                groups[groups.count - 1].kind = .answers(answers)
            } else {
                groups.append(LetterGroup(at: at, kind: .answers([letter])))
            }
        }
        return groups
    }
}

/// The parts of an agent's reply, calls gathered into runs. A streaming turn's older thoughts fold behind one line.
struct Parts: View {
    @Environment(\.palette) private var palette
    @Environment(TranscriptContext.self) private var context
    let id: String
    let parts: [ChatPart]
    let live: Bool

    var body: some View {
        let groups = PartGroup.of(parts)
        let thinks = parts.contains { if case .thought = $0 { true } else { false } }
        let earlier = thinks ? context.earlierThoughts : .none
        let earlierOpen = !earlier.ids.isEmpty && context.folds.isOpen(earlier.key, false)
        VStack(alignment: .leading, spacing: 0) {
            ForEach(Array(groups.enumerated()), id: \.element.id) { index, group in
                let last = live && index == groups.count - 1
                switch group {
                case .tools(let first, let tools):
                    ToolGroup(id: "\(id)/\(first)", parts: tools, live: last)
                case .part(.thought(let thought)):
                    let place = earlier.ids.firstIndex(of: thought.id)
                    if place == 0 {
                        EarlierThinking(count: earlier.ids.count, open: earlierOpen) {
                            context.folds.set(earlier.key, !earlierOpen)
                        }
                    }
                    if place == nil || earlierOpen {
                        PartView(part: .thought(thought), last: last)
                    }
                case .part(let part):
                    PartView(part: part, last: last).padding(.top, index > 0 && part.isText ? 10 : 0)
                }
            }
        }
    }
}

/// The line a streaming turn's older thoughts fold behind, which opens them in place.
private struct EarlierThinking: View {
    @Environment(\.palette) private var palette
    let count: Int
    let open: Bool
    let toggle: () -> Void

    var body: some View {
        let thoughts = "\(count) \(count == 1 ? "thought" : "thoughts")"
        Button(action: toggle) {
            HStack(spacing: 8) {
                IconView(icon: Icons.chevron, size: 11).foregroundStyle(palette.inkDim)
                    .rotationEffect(.degrees(open ? 90 : 0))
                Text("Earlier thinking").font(Theme.ui(12.5)).foregroundStyle(palette.inkDim)
                Text(thoughts).font(Theme.ui(12.5)).foregroundStyle(palette.inkFaint)
            }
            .frame(height: 26)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Earlier thinking, \(thoughts)")
        .accessibilityValue(open ? "expanded" : "collapsed")
    }
}

extension ChatPart {
    fileprivate var isText: Bool {
        guard case .text(let text) = self else { return false }
        return !text.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }
}

private struct PartView: View {
    @Environment(\.palette) private var palette
    let part: ChatPart
    let last: Bool

    var body: some View {
        switch part {
        case .text(let text):
            if !text.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                LongText(id: text.id, text: text.text, md: text.md, live: last, style: .reply(palette))
            }
        case .thought(let thought):
            let words = thought.text.trimmingCharacters(in: .whitespacesAndNewlines)
            if !words.isEmpty {
                LongText(id: thought.id, text: words, md: thought.md, live: last, style: .thought(palette))
                    .padding(.vertical, 6)
            }
        case .subagent(let subagent):
            SubagentRow(subagent: subagent.subagent)
        case .notice(let notice):
            TaskNoticeOrCard(id: notice.id, notice: notice.notice)
        case .content(let content):
            ContentView(block: content.content)
        case .agentNotice(let notice):
            AgentNoticeLine(notice: notice.notice)
        case .account(let account):
            AccountLine(move: account.move)
        case .tool(let tool):
            if let page = tool.page { PageCard(page: page) }
        case .unknown:
            EmptyView()
        }
    }
}

/// A message too long to draw at once shows its start and the rest on a tap, unless the person watched it stream in.
struct LongText: View {
    @Environment(\.palette) private var palette
    @Environment(TranscriptContext.self) private var context
    let id: String
    let text: String
    let md: PartTree?
    let live: Bool
    let style: ProseStyle

    private static let limit = 20_000

    var body: some View {
        let blocks = md?.blocks ?? plainBlocks(text)
        let found = context.finding?.marks(id)
        let watched = context.folds.watched(id, live: live)
        let whole = context.folds.isOpen("whole/\(id)", false)
        let cut = whole || found != nil || watched ? nil : Self.cut(blocks)
        let plain = Array(blocks.map { visibleText($0) }.joined().utf16)
        VStack(alignment: .leading, spacing: 0) {
            if live && cut == nil && found == nil {
                let newest = context.reveal.newest(id, text: plain, live: live)
                TimelineView(.explicit([newest.until])) { timeline in
                    MarkdownView(
                        blocks: blocks, style: style, fading: timeline.date < newest.until ? newest.fading : [])
                }
            } else {
                MarkdownView(blocks: cut?.blocks ?? blocks, style: style, marks: found)
            }
            if let cut {
                Button {
                    context.folds.set("whole/\(id)", true)
                } label: {
                    Text("Show the rest · \(Int((Double(cut.hidden) / 1000).rounded()))k more characters")
                        .font(Theme.ui(12.5, .medium))
                        .foregroundStyle(palette.tertiary)
                }
                .buttonStyle(.plain)
                .padding(.top, 6)
                .padding(.bottom, 4)
            }
        }
    }

    /// A long answer's first whole blocks, or the start of its first block when that alone is too long.
    static func cut(_ blocks: [MdElement]) -> (blocks: [MdElement], hidden: Int)? {
        let lengths = blocks.map { visibleText($0).count }
        let total = lengths.reduce(0, +)
        guard total > limit else { return nil }
        var kept = 0
        var count = 0
        while count < blocks.count, kept + lengths[count] <= limit {
            kept += lengths[count]
            count += 1
        }
        let head = count > 0 ? Array(blocks.prefix(count)) : [cutBlock(blocks[0], limit)]
        let hidden = total - (count > 0 ? kept : limit)
        return hidden < limit / 10 ? nil : (head, hidden)
    }

    private static func cutBlock(_ block: MdElement, _ chars: Int) -> MdElement {
        switch block {
        case .pre(var pre):
            pre.v = String(pre.v.prefix(chars))
            return .pre(pre)
        default:
            return .p(.init(c: [.text(String(visibleText(block).prefix(chars)))]))
        }
    }
}

/// A finished answer's work, folded above the reply it closed with, as on the Mac.
struct WorkSummary: View {
    @Environment(\.palette) private var palette
    @Environment(TranscriptContext.self) private var context
    let took: Int?
    let calls: Int
    let thought: Bool
    let open: Bool
    let onToggle: () -> Void

    var body: some View {
        let ink = palette.ink.mixed(with: palette.inkDim, 0.6)
        let worked = ChatWords.worked(took, thought: thought)
        VStack(alignment: .leading, spacing: 0) {
            Button {
                Haptics.select()
                onToggle()
            } label: {
                HStack(spacing: 8) {
                    AgentMark(provider: context.provider, size: 17)
                    Text(worked)
                    if calls > 0 {
                        Text("·").padding(.horizontal, -2)
                        Text(ChatWords.calls(calls))
                    }
                    IconView(icon: Icons.chevron, size: 10).opacity(0.7).rotationEffect(.degrees(open ? 90 : 0))
                }
                .font(Theme.ui(12.5))
                .foregroundStyle(ink)
                .frame(height: 24)
                .padding(.horizontal, 6)
                .contentShape(Rectangle())
            }
            .buttonStyle(ActiveFill(radius: 6))
            .padding(.leading, -6)
            .accessibilityLabel(calls > 0 ? "\(worked), \(ChatWords.calls(calls))" : worked)
            .accessibilityValue(open ? "expanded" : "collapsed")
            Rectangle().fill(palette.ink.opacity(0.14)).frame(height: 1).padding(.top, 4)
        }
        .padding(.top, 8)
        .padding(.bottom, 4)
    }
}

/// The one line a run of messages between agents folds to, as on the Mac: a tap opens it where it is.
struct TeamFoldLine: View {
    @Environment(\.palette) private var palette
    let line: TeamLine
    let open: Bool
    let all: Bool
    let onToggle: () -> Void
    let onAll: () -> Void

    private var words: (String, Bool) {
        guard let need = line.need else { return (line.latest.map { "@\($0.name): \($0.text)" } ?? "", false) }
        let name = "@\(need.name)"
        let said: String =
            switch need.kind {
            case .asks: need.text.map { "\(name) asks: \($0)" } ?? "\(name) has a question"
            case .waits: need.text.map { "\(name): \($0)" } ?? "\(name) waits on you"
            case .failed: need.text.map { "\(name) failed: \($0)" } ?? "\(name) failed"
            default: "\(name) stopped"
            }
        return (said, true)
    }

    var body: some View {
        let (said, wanting) = words
        let count = line.messages == 1 ? "1 message" : "\(line.messages) messages"
        VStack(alignment: .leading, spacing: 0) {
            Button {
                Haptics.select()
                onToggle()
            } label: {
                HStack(spacing: 6) {
                    IconView(icon: Icons.chevron, size: 10)
                        .foregroundStyle(palette.ink.mixed(with: palette.inkDim, 0.6))
                        .opacity(0.7)
                        .rotationEffect(.degrees(open ? 90 : 0))
                    HStack(spacing: 2) {
                        ForEach(Array(line.agents.enumerated()), id: \.offset) { _, agent in
                            AgentFaceView(agentId: agent.agentId, provider: agent.cli, size: 16)
                        }
                        if line.more > 0 {
                            Text("+\(line.more)").font(Theme.ui(11)).foregroundStyle(palette.inkDim).padding(
                                .leading, 2)
                        }
                    }
                    Text(count).font(Theme.ui(12.5, .semibold)).foregroundStyle(palette.ink)
                    Text(said).font(Theme.ui(12.5)).foregroundStyle(wanting ? palette.danger : palette.inkDim)
                        .lineLimit(1)
                        .frame(maxWidth: .infinity, alignment: .leading)
                    if let took = line.took {
                        Text(Self.took(took)).font(Theme.ui(12)).monospacedDigit().foregroundStyle(palette.inkDim)
                    }
                }
                .frame(height: 28)
                .padding(.horizontal, 6)
                .contentShape(Rectangle())
            }
            .buttonStyle(ActiveFill(radius: 6))
            .padding(.horizontal, -6)
            .accessibilityLabel("\(count). \(said)")
            .accessibilityValue(open ? "expanded" : "collapsed")
            if open, !all, line.earlier > 0 {
                Button(action: onAll) {
                    Text("Show \(line.earlier) earlier").font(Theme.ui(12)).foregroundStyle(palette.inkDim)
                        .padding(.horizontal, 6).frame(height: 24)
                }
                .buttonStyle(ActiveFill(radius: 6))
                .padding(.top, 4)
                .padding(.leading, 12)
            }
        }
        .padding(.top, 6)
        .padding(.bottom, 2)
    }

    /// How long a run lasted, to the minute once it passes one: `41m`, `2h 6m`.
    static func took(_ ms: Int) -> String {
        if ms < 60_000 { return ChatWords.duration(ms: Double(ms)) }
        let minutes = Int((Double(ms) / 60_000).rounded())
        return minutes < 60 ? "\(minutes)m" : "\(minutes / 60)h \(minutes % 60)m"
    }
}
