import SwiftUI

/// The question an agent asked that whatever is sent answers.
struct ComposerQuestion {
    let name: String
    let cli: String
    let text: String
    let onRemove: () -> Void
    let onAnswer: (WrittenAnswer) async -> Bool
}

/// The bottom of a chat: what waits on the person docked above the composer, the menus over it, and the composer
/// with its bar of attach, stash, YOLO, model, effort and context, and send or stop.
struct Composer: View {
    @Environment(\.palette) private var palette
    let session: ChatSession
    /// The host the chat is on.
    let core: String
    let provider: String
    let running: Bool
    /// Sending while the turn runs steers into it rather than waiting behind it.
    let steers: Bool
    let placeholder: String
    /// The host would refuse a switch between YOLO and safe now.
    let modeLocked: Bool
    let watchOnly: Bool
    /// The host is out of reach, so a message is kept until it is back.
    let offline: Bool
    let hostName: String
    let onSent: () -> Void
    /// What is still going, docked on the composer's top edge while nothing more urgent is.
    var strip: AnyView?
    /// Pulling an empty composer up opens what was sent before; absent when nothing was.
    var onRecent: (() -> Void)?
    var question: ComposerQuestion?
    let focus: ComposerFocus

    @State private var adding: Adding?
    @State private var configuring = false
    @State private var focused = false
    @State private var holds = Holds()
    @State private var mentions: MentionSearch?
    @State private var prompt = PromptStash()

    private enum Adding { case files, stash }

    /// How far up an empty composer is pulled before it opens the recent prompts.
    private static let pullUp: CGFloat = 18

    var body: some View {
        VStack(spacing: 0) {
            if watchOnly {
                dock ?? strip
                HStack(spacing: 10) {
                    IconView(icon: Icons.eye, size: 17).foregroundStyle(palette.inkDim)
                    Text("Watching").font(Theme.ui(13.5)).foregroundStyle(palette.inkDim).frame(maxWidth: .infinity, alignment: .leading)
                }
                .padding(14)
                .background(composerShape.fill(palette.composer))
                .overlay(composerShape.strokeBorder(palette.border, lineWidth: 1))
            } else {
                writing
            }
        }
        .padding(.horizontal, 10)
        .padding(.top, 8)
        .padding(.bottom, 6)
        .task(id: session.agentId) { mentions = MentionSearch(core: core, session: session) }
    }

    private var composerShape: RoundedRectangle { RoundedRectangle(cornerRadius: 16, style: .continuous) }

    private var holdsHere: [ShipHold] {
        (Hub.shared.live(core).view?.inbox.items ?? []).filter { $0.agentId == session.agentId }.compactMap(\.hold)
    }

    private var dock: AnyView? {
        let requests = session.permissions
        if !requests.isEmpty {
            return AnyView(PermissionDock(requests: requests, provider: provider, answering: session.answering, onAnswer: session.answer))
        }
        let holds = holdsHere
        if holds.isEmpty || watchOnly { return nil }
        return AnyView(HoldDock(holds: holds, answering: self.holds.answering) { hold, how in
            self.holds.answer(hold, how, core: core, report: session.report)
        })
    }

    @ViewBuilder
    private var writing: some View {
        let rows = session.rows
        let draft = session.draft!
        let text = draft.text
        let matches = Self.matching(rows.commands, text)
        let stashQuery = matches.isEmpty ? StashWords.query(in: text) : nil
        let typed = Mentions.typed(text, draft.tokens)
        let dock = dock
        let attachments = draft.attachments
        let chips = draft.chips
        let uploading = draft.uploading
        let room = WireLimits.maxAttachments - attachments.count
        let sendable = !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || !attachments.isEmpty || !chips.isEmpty

        dock
        if !matches.isEmpty { commands(matches) }
        if let stashQuery {
            StashMenu(query: stashQuery) { item in fromStash([item], base: StashWords.withoutQuery(text)) }
        }
        if let typed, let search = mentions {
            let found = search.rows(typed)
            MentionMenu(typed: typed, rows: found.rows, empty: found.empty) { trigger in
                let at = typed.start
                session.setDraft(text.units(0, at) + trigger.rawValue + text.units(at + 1, (text as NSString).length))
            }
            .onAppear { search.ask(typed) }
            .onChange(of: typed) { _, now in search.ask(now) }
        }
        if let toast = prompt.toast {
            StashedToast(done: toast != .stashing) {
                prompt.undo({ id in try await session.unstashPrompt(id) }, report: session.report)
            }
        }
        if dock == nil && matches.isEmpty && stashQuery == nil && typed == nil { strip }
        VStack(spacing: 0) {
            if let question { answering(question) }
            if !attachments.isEmpty || !chips.isEmpty {
                ComposerAttachments(attachments: attachments, core: core, onRemove: session.removeAttachment, onRetry: send) {
                    ForEach(chips, id: \.uri) { chip in
                        StashChip(chip: chip) { session.removeChip(chip.uri) }
                    }
                }
            }
            ComposerInput(
                text: Binding(get: { session.draftText }, set: { session.setDraft($0) }),
                tokens: draft.tokens, placeholder: placeholder, editable: !uploading, focus: focus,
                onFocus: { focused = $0 }
            )
            bar(rows: rows, sendable: sendable, uploading: uploading, room: room)
        }
        .padding(6)
        .background(composerShape.fill(palette.composer))
        .overlay(composerShape.strokeBorder(focused ? palette.borderSelected : palette.border, lineWidth: 1))
        .simultaneousGesture(
            DragGesture(minimumDistance: Self.pullUp).onChanged { drag in
                guard text.isEmpty, let onRecent, -drag.translation.height > Self.pullUp,
                      -drag.translation.height > abs(drag.translation.width) * 2 else { return }
                Haptics.select()
                onRecent()
            },
            including: text.isEmpty && onRecent != nil ? .all : .subviews
        )
        .attachSheet(
            shown: Binding(get: { adding != nil }, set: { if !$0 { adding = nil } }),
            stash: adding == .stash, room: room,
            onAttach: { session.attach($0) },
            onStash: { fromStash($0) },
            onStashPrompt: sendable && !uploading ? { stashPrompt() } : nil,
            onProblem: { session.report($0) }
        )
        .liftedSheet(shown: $configuring) {
            ConfigSheet(provider: provider, model: rows.model, effort: rows.effort, usage: rows.usage) { picker, value in
                session.setConfig(picker.id, value)
                configuring = false
            }
        }
    }

    private func bar(rows: ChatRows, sendable: Bool, uploading: Bool, room: Int) -> some View {
        let model = Self.label(rows.model)
        let effort = Self.label(rows.effort)
        return HStack(spacing: 2) {
            Button { adding = .files } label: {
                IconView(icon: Icons.plus, size: 17).foregroundStyle(palette.inkDim).frame(width: 34, height: 34).contentShape(Circle())
            }
            .buttonStyle(ActiveFill(radius: 17))
            .disabled(uploading || room <= 0)
            .opacity(uploading || room <= 0 ? 0.4 : 1)
            .accessibilityLabel("Add photos or files")
            StashButton { adding = .stash }
            YoloToggle(core: core, mode: session.permissionMode.flatMap(PermissionMode.init(rawValue:)), provider: provider, locked: modeLocked, onToggle: session.setPermissionMode)
            if let model {
                Capped(width: 160) { picker(model, accent: true, label: "Model, \(model)") }
            }
            if let effort {
                picker(effort, accent: false, label: "Effort, \(effort)").fixedSize()
            }
            if let usage = rows.usage {
                ContextRing(usage: usage, provider: provider) { configuring = true }
            }
            Spacer(minLength: 0)
            sendOrStop(sendable: sendable, uploading: uploading)
        }
        .padding(.top, 6)
    }

    private func picker(_ value: String, accent: Bool, label: String) -> some View {
        Button { configuring = true } label: {
            HStack(spacing: 6) {
                if accent { AgentMark(provider: provider, size: 16) }
                Text(value).font(Theme.ui(12.5)).foregroundStyle(accent ? palette.accent : palette.inkDim).lineLimit(1)
                IconView(icon: Icons.chevron, size: 10)
                    .foregroundStyle(accent ? palette.accent : palette.inkDim)
                    .rotationEffect(.degrees(90))
                    .opacity(0.6)
            }
            .padding(.horizontal, 7)
            .frame(height: 34)
            .contentShape(Rectangle())
        }
        .buttonStyle(ActiveFill(radius: 7))
        .accessibilityLabel(label)
    }

    @ViewBuilder
    private func sendOrStop(sendable: Bool, uploading: Bool) -> some View {
        if sendable || !running {
            Button(action: send) {
                Group {
                    if uploading {
                        ProgressView().tint(palette.ground).controlSize(.small)
                    } else {
                        IconView(icon: Icons.arrowUp, size: 16).foregroundStyle(palette.ground)
                    }
                }
                .frame(width: 34, height: 34)
                .background(Circle().fill(palette.ink))
                .opacity(!sendable || offline ? 0.28 : 1)
            }
            .buttonStyle(PressFade())
            .disabled(uploading)
            .simultaneousGesture(LongPressGesture().onEnded { _ in if sendable { stashPrompt() } })
            .accessibilityLabel(uploading ? "Sending files" : offline ? "Send, waiting for \(hostName)" : running ? (steers ? "Send to steer this turn" : "Send after this turn") : "Send")
            .accessibilityHint(sendable ? "Hold to stash this prompt" : "")
        } else {
            Button(action: session.cancel) {
                RoundedRectangle(cornerRadius: 2, style: .continuous)
                    .fill(palette.ground)
                    .frame(width: 10, height: 10)
                    .frame(width: 34, height: 34)
                    .background(Circle().fill(palette.ink))
            }
            .buttonStyle(PressFade())
            .accessibilityLabel("Stop")
        }
    }

    private func commands(_ matches: [ChatCommandInfo]) -> some View {
        VStack(spacing: 4) {
            ForEach(matches, id: \.name) { command in
                Button { session.setDraft("/\(command.name) ") } label: {
                    HStack(spacing: 10) {
                        Text("/\(command.name)").font(Theme.mono(13)).foregroundStyle(palette.accent)
                        Text(command.description).font(Theme.ui(13)).foregroundStyle(palette.tertiary).lineLimit(1)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    .padding(.horizontal, 12)
                    .frame(minHeight: 40)
                    .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(palette.overlay))
                    .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(palette.border, lineWidth: 1))
                    .contentShape(Rectangle())
                }
                .buttonStyle(PressFade())
                .accessibilityLabel("/\(command.name), \(command.description)")
            }
        }
        .padding(.bottom, 8)
    }

    private func answering(_ question: ComposerQuestion) -> some View {
        HStack(spacing: 7) {
            AgentMark(provider: question.cli, size: 13)
            Text("Answering \(question.name) · \(question.text)").font(Theme.ui(12)).foregroundStyle(palette.inkDim).lineLimit(1)
            Button(action: question.onRemove) {
                IconView(icon: Icons.close, size: 10)
                    .foregroundStyle(palette.ground)
                    .frame(width: 20, height: 20)
                    .background(Circle().fill(palette.ink))
                    .contentShape(Rectangle().inset(by: -8))
            }
            .buttonStyle(PressFade())
            .accessibilityLabel("Stop answering \(question.name)")
        }
        .padding(.vertical, 6)
        .padding(.leading, 9)
        .padding(.trailing, 6)
        .background(RoundedRectangle(cornerRadius: 9, style: .continuous).fill(palette.sunken))
        .overlay(RoundedRectangle(cornerRadius: 9, style: .continuous).strokeBorder(palette.border, lineWidth: 1))
        .padding(4)
        .padding(.bottom, -2)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func send() {
        let text = session.draftText.trimmingCharacters(in: .whitespacesAndNewlines)
        let draft = session.draft!
        let sendable = !text.isEmpty || !draft.attachments.isEmpty || !draft.chips.isEmpty
        guard sendable, !offline, !draft.uploading else { return }
        if let unknown = Self.unknownCommand(text, session.rows.commands) {
            session.report("/\(unknown) is not available in this session")
            return
        }
        Haptics.tap()
        if let question {
            Task {
                if await session.answerWith(text, question.onAnswer) { sent() }
            }
        } else if session.send(text, steers: steers) {
            sent()
        }
    }

    private func sent() {
        session.setDraft("")
        onSent()
    }

    private func stashPrompt() {
        prompt.stash({
            let id = try await session.stashPrompt()
            focus.clear()
            return id
        }, report: session.report)
    }

    private func fromStash(_ items: [StashItem], base: String? = nil) {
        if let base, base != session.draftText { session.setDraft(base) }
        Task {
            do {
                try await session.attachStash(items.map(\.id))
            } catch {
                session.report("could not attach it: \(problem(error))")
            }
        }
    }

    /// The name of the value a picker holds, or the value itself when its choices do not list it.
    static func label(_ picker: ConfigPicker?) -> String? {
        guard let picker else { return nil }
        if let found = picker.choices.first(where: { $0.value == picker.current }) { return found.label }
        return picker.current.isEmpty ? nil : picker.current
    }

    /// The agent's slash commands with what is typed anywhere in their name, while only a command is typed.
    static func matching(_ commands: [ChatCommandInfo], _ typed: String) -> [ChatCommandInfo] {
        guard typed.hasPrefix("/"), !typed.contains(where: \.isWhitespace) else { return [] }
        let needle = typed.dropFirst().lowercased()
        return Array(commands.filter { needle.isEmpty || $0.name.lowercased().contains(needle) }.prefix(8))
    }

    /// The command a message opens with that the session does not offer. A session that lists none is taken at its
    /// word, and a path such as `/usr/bin` goes to the agent as text.
    static func unknownCommand(_ text: String, _ commands: [ChatCommandInfo]) -> String? {
        guard text.hasPrefix("/"), !commands.isEmpty else { return nil }
        let word = text.dropFirst().prefix { !$0.isWhitespace }
        guard !word.isEmpty, word.allSatisfy({ $0.isLetter || $0.isNumber || "_:.-".contains($0) }) else { return nil }
        let name = String(word)
        return commands.contains { $0.name == name } ? nil : name
    }
}

/// Its content at its own width, but never wider than `width`, as a capped flexible box sits in a row.
private struct Capped: Layout {
    let width: CGFloat

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        subviews.first?.sizeThatFits(capped(proposal)) ?? .zero
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        subviews.first?.place(at: bounds.origin, proposal: ProposedViewSize(bounds.size))
    }

    private func capped(_ proposal: ProposedViewSize) -> ProposedViewSize {
        ProposedViewSize(width: min(proposal.width ?? width, width), height: proposal.height)
    }
}
