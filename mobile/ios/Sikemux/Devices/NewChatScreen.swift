import SikemuxCore
import SwiftUI

/// The account a launcher runs on, as the core names it after the agent: "Claude · Work" is Work.
@MainActor private func accountName(_ launcher: LauncherInfo, core: String) -> String {
    let prefix = "\(Catalog.label(launcher.provider, core: core)) · "
    return launcher.label.hasPrefix(prefix) ? String(launcher.label.dropFirst(prefix.count)) : launcher.label
}

/// A new chat: the agent and project it starts with, and its first message.
struct NewChatScreen: View {
    let core: String
    var project: String?
    var launcher: String?

    var body: some View {
        NewChatContent(core: core, projectId: project, launcherId: launcher).watching(core)
    }
}

private struct NewChatContent: View {
    enum SheetShown { case agent, project, settings, attach, stash }

    /// What the screen held when it was left, so coming back to it finds it as it was.
    struct Kept: Codable {
        var text: String
        var attachments: [Attachment]
        var chips: [StashItem]
    }

    /// How long typing rests before the draft is written down.
    private static let keepAfter: Duration = .milliseconds(400)

    /// What was sent, drawn as the chat will draw it while the agent starts.
    struct Sending {
        let message: ChatMessage
        let draft: String
        let attachments: [Attachment]
        let since: Date
    }

    private static let idle = "Ask about this project, or type / for commands"

    @Environment(\.palette) private var palette
    let core: String
    @State var projectId: String?
    @State var launcherId: String?
    @State private var chosenMode: PermissionMode?
    @State private var draft = ""
    @State private var attachments: [Attachment] = []
    @State private var chips: [StashItem] = []
    @State private var prompt = PromptStash()
    /// What the composer held when its prompt went to the stash, to put back on Undo.
    @State private var stashedFrom: Kept?
    @State private var heldStart = false
    @State private var sheet: SheetShown?
    @State private var sending: Sending?
    @State private var context: TranscriptContext?
    @State private var problem: String?
    /// The agent an earlier try started, sent to again rather than starting another.
    @State private var started: String?
    /// Where the first message is while the host makes its worktree and starts the agent there.
    @State private var step: String?
    /// The host made this chat's worktree, so sending again starts the agent there rather than making another.
    @State private var inWorktree = false
    /// What the person flipped the worktree switch to for a project, until the host's own view says so.
    @State private var flipped: (project: String, on: Bool)?

    var body: some View {
        let live = Hub.shared.live(core)
        let workspace = live.view?.workspace
        let launcher = workspace.flatMap { space in
            space.launchers.first { $0.id == launcherId } ?? space.launchers.first
        }
        let accounts = workspace?.launchers.filter { $0.provider == launcher?.provider } ?? []
        let project = workspace.flatMap { space in space.projects.first { $0.id == projectId } ?? space.projects.first }
        let worktree = (flipped?.project == project?.id ? flipped?.on : nil) ?? project?.worktreeDefault ?? false
        let starting = sending != nil
        let sendable = written && launcher != nil && project != nil && live.isOpen && !starting
        let blocked: String? =
            !live.isOpen
            ? (live.view != nil ? "Reconnecting to the host…" : "Not connected to the host yet.")
            : workspace != nil && project == nil ? "No open projects on the host." : nil
        let noAgents = workspace.map { $0.launchers.isEmpty } ?? false

        Screen {
            Nav(
                title: {
                    if let launcher {
                        ChatTitle(
                            agentId: nil, provider: launcher.provider, name: nil,
                            title: "New \(Catalog.label(launcher.provider, core: core)) chat")
                    }
                },
                end: {
                    if accounts.count > 1 && started == nil {
                        IconButton(icon: Icons.moreVertical, label: "Chat settings") { sheet = .settings }
                    }
                })
            if let sending, let launcher, let context {
                VStack(alignment: .leading, spacing: 12) {
                    Spacer()
                    TranscriptMessage(message: sending.message, look: RowLook(unsent: Unsent(state: .sending)))
                    ActivityLine(provider: launcher.provider, label: step ?? "Starting", since: sending.since)
                }
                .environment(context)
                .padding(.horizontal, 18)
                .padding(.bottom, 28)
            } else {
                VStack(spacing: 12) {
                    if noAgents {
                        PromptGlyph(icon: Icons.agentLine, small: true)
                    } else if live.isOpen, workspace != nil, project == nil {
                        PromptGlyph(icon: Icons.folder, small: true)
                    } else if let launcher {
                        AgentFaceView(agentId: project?.id, provider: launcher.provider, size: 38, logoSize: 40)
                    }
                    Text(project?.name ?? "New chat").font(Theme.ui(20, .semibold)).tracking(-0.55).foregroundStyle(
                        palette.ink)
                    if noAgents { Text("No agents on the host.").metaStyle(palette).multilineTextAlignment(.center) }
                    if let problem {
                        Text(problem).font(Theme.ui(13.5)).foregroundStyle(palette.danger).multilineTextAlignment(
                            .center)
                    } else if let blocked {
                        Text(blocked).metaStyle(palette).multilineTextAlignment(.center)
                    }
                }
                .padding(.horizontal, 28)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
            composer(launcher: launcher, project: project, worktree: worktree, sendable: sendable, starting: starting)
        }
        .sheet(shown: Binding(get: { sheet == .agent }, set: { if !$0 { sheet = nil } })) {
            AgentPicker(launchers: workspace?.launchers ?? [], chosen: launcher) {
                launcherId = $0.id
                chosenMode = nil
                sheet = nil
            }
        }
        .sheet(shown: Binding(get: { sheet == .settings }, set: { if !$0 { sheet = nil } })) {
            if let launcher {
                AccountPicker(accounts: accounts, chosen: launcher) {
                    launcherId = $0.id
                    sheet = nil
                }
            }
        }
        .onAppear(perform: restore)
        .task(id: keptPrint) {
            do {
                try await Task.sleep(for: Self.keepAfter)
                keep()
            } catch {
                return
            }
        }
        .onDisappear { if sending == nil { keep() } }
        .attachSheet(
            shown: Binding(get: { sheet == .attach || sheet == .stash }, set: { if !$0 { sheet = nil } }),
            stash: sheet == .stash, room: WireLimits.maxAttachments - attachments.count,
            onAttach: attach, onStash: { fromStash($0) }, onStashPrompt: stashable ? { stashPrompt() } : nil,
            onProblem: { problem = $0 }
        )
        .sheet(
            shown: Binding(get: { sheet == .project }, set: { if !$0 { sheet = nil } }),
            tall: (workspace?.projects.count ?? 0) > 6
        ) {
            ProjectSheet(projects: workspace?.projects ?? [], chosen: project?.id) {
                if let next = $0 { projectId = next }
                sheet = nil
            }
        }
    }

    @ViewBuilder
    private func composer(
        launcher: LauncherInfo?, project: ProjectInfo?, worktree: Bool, sendable: Bool, starting: Bool
    ) -> some View {
        VStack(spacing: 0) {
            if let project {
                HStack {
                    Button {
                        sheet = .project
                    } label: {
                        HStack(spacing: 7) {
                            IconView(icon: Icons.folder, size: 13).foregroundStyle(palette.live)
                            Text(project.name).font(Theme.ui(13.5, .medium)).foregroundStyle(palette.ink)
                            IconView(icon: Icons.chevron, size: 10).foregroundStyle(palette.inkFaint).rotationEffect(
                                .degrees(90))
                        }
                        .padding(.vertical, 4)
                    }
                    .buttonStyle(PressFade())
                    .disabled(started != nil)
                    .accessibilityLabel("Project")
                    Spacer()
                    if project.git != nil {
                        WorktreeSwitch(on: worktree || inWorktree, disabled: starting || inWorktree) {
                            flipWorktree(project, worktree)
                        }
                    }
                }
                .padding(.vertical, 5)
                .padding(.leading, 12)
                .padding(.trailing, 6)
                .background(
                    UnevenRoundedRectangle(topLeadingRadius: 12, topTrailingRadius: 12, style: .continuous).fill(
                        palette.composer)
                )
                .overlay(
                    UnevenRoundedRectangle(topLeadingRadius: 12, topTrailingRadius: 12, style: .continuous)
                        .stroke(palette.border, lineWidth: 1)
                        .mask(Rectangle().padding(.bottom, 1))
                )
                .padding(.horizontal, 14)
                .padding(.bottom, -1)
            }
            if let toast = prompt.toast {
                StashedToast(done: toast != .stashing) {
                    prompt.undo(
                        { id in
                            if let held = stashedFrom { putBack(held) }
                            try await StashAPI.delete(id)
                        }, report: { problem = $0 })
                }
            }
            if let query = StashWords.query(in: draft) {
                StashMenu(query: query) { fromStash([$0], base: StashWords.withoutQuery(draft)) }
            }
            VStack(spacing: 0) {
                if !attachments.isEmpty || !chips.isEmpty {
                    ComposerAttachments(
                        attachments: attachments,
                        onRemove: { id in
                            if !starting { attachments.removeAll { $0.id == id } }
                        }, onRetry: { start(launcher: launcher, project: project, worktree: worktree) }
                    ) {
                        ForEach(chips, id: \.id) { chip in
                            StashChip(chip: StashWords.contextChip(chip)) { chips.removeAll { $0.id == chip.id } }
                        }
                    }
                }
                ComposerInput(text: $draft, placeholder: Self.idle, editable: !starting, focusOnShow: true)
                HStack(spacing: 2) {
                    let room = WireLimits.maxAttachments - attachments.count
                    Button {
                        sheet = .attach
                    } label: {
                        IconView(icon: Icons.plus, size: 17).foregroundStyle(palette.inkDim).frame(
                            width: 34, height: 34
                        ).contentShape(Circle())
                    }
                    .buttonStyle(RowPressStyle())
                    .disabled(starting || room <= 0)
                    .opacity(starting || room <= 0 ? 0.4 : 1)
                    .accessibilityLabel("Add photos or files")
                    StashButton { sheet = .stash }
                    if let launcher {
                        YoloToggle(
                            core: core, mode: chosenMode ?? launcher.permissionMode, provider: launcher.provider,
                            locked: starting || started != nil
                        ) { chosenMode = $0 }
                        Button {
                            sheet = .agent
                        } label: {
                            HStack(spacing: 6) {
                                AgentMark(provider: launcher.provider, size: 16)
                                Text(Catalog.label(launcher.provider, core: core)).font(Theme.ui(12.5)).foregroundStyle(
                                    palette.accent)
                                IconView(icon: Icons.chevron, size: 10).foregroundStyle(palette.accent).rotationEffect(
                                    .degrees(90)
                                ).opacity(0.6)
                            }
                            .padding(.horizontal, 7)
                            .frame(height: 34)
                        }
                        .buttonStyle(PressFade())
                        .disabled(started != nil)
                        .accessibilityLabel("Agent")
                    }
                    Spacer()
                    Button {
                        // A finger lifted after holding Start has already stashed the prompt.
                        if heldStart {
                            heldStart = false
                        } else {
                            start(launcher: launcher, project: project, worktree: worktree)
                        }
                    } label: {
                        Group {
                            if starting {
                                Working()
                            } else {
                                IconView(icon: Icons.arrowUp, size: 16).foregroundStyle(palette.ground)
                            }
                        }
                        .frame(width: 34, height: 34)
                        .background(Circle().fill(palette.ink))
                        .opacity(sendable ? 1 : 0.28)
                    }
                    .buttonStyle(PressFade())
                    .simultaneousGesture(
                        LongPressGesture().onEnded { _ in
                            guard stashable else { return }
                            heldStart = true
                            stashPrompt()
                        }
                    )
                    .disabled(!sendable && !stashable)
                    .accessibilityLabel("Start")
                }
                .padding(.top, 6)
            }
            .padding(6)
            .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(palette.composer))
            .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).strokeBorder(palette.border, lineWidth: 1))
        }
        .padding(.horizontal, 10)
        .padding(.top, 8)
        .padding(.bottom, 6)
    }

    private var written: Bool {
        !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || !attachments.isEmpty || !chips.isEmpty
    }

    private var stashable: Bool { written && sending == nil }

    private var keptKey: String { "\(core):new" }

    private var kept: Kept? {
        written ? Kept(text: draft, attachments: attachments, chips: chips) : nil
    }

    /// Changes whenever what would be kept does, so the draft is written down once typing rests.
    private var keptPrint: String {
        "\(draft)|\(attachments.map(\.id))|\(chips.map(\.id))"
    }

    private func restore() {
        guard draft.isEmpty, attachments.isEmpty, chips.isEmpty,
            let held = DraftStore.load(keptKey, as: Kept.self)
        else { return }
        draft = held.text
        attachments = held.attachments.filter { FileManager.default.fileExists(atPath: $0.uri.path) }
        chips = held.chips
    }

    private func keep() {
        if let kept { DraftStore.save(keptKey, kept) } else { DraftStore.clear(keptKey) }
    }

    /// Stashes what the composer holds as one prompt and empties it; Undo puts it back.
    private func stashPrompt() {
        let held = Kept(text: draft, attachments: attachments, chips: chips)
        stashedFrom = held
        draft = ""
        attachments = []
        chips = []
        prompt.stash(
            {
                let words = StashWords.withContext(
                    held.text.trimmingCharacters(in: .whitespacesAndNewlines), held.chips.map(StashWords.contextChip))
                let text = words.isEmpty ? held.attachments.map(\.name).joined(separator: ", ") : words
                do {
                    let files = held.attachments.map { StashFile(url: $0.uri, name: $0.name, mime: $0.mime) }
                    return try await StashAPI.stashPrompt(text: text, files: files).id
                } catch {
                    putBack(held)
                    throw error
                }
            }, report: { problem = $0 })
    }

    private func putBack(_ held: Kept) {
        draft = [held.text, draft].filter { !$0.isEmpty }.joined(separator: "\n\n")
        attachments = held.attachments + attachments
        chips = held.chips + chips.filter { chip in !held.chips.contains { $0.id == chip.id } }
    }

    private func attach(_ picked: [Attachment]) {
        let (fits, notice) = Uploads.fitting(held: attachments.count, picked)
        if let notice { problem = notice }
        attachments += fits
    }

    /// Stash items into the message: notes into its words, links and text as chips, files fetched to go with it.
    private func fromStash(_ items: [StashItem], base: String? = nil) {
        let put = StashWords.intoMessage(items, draft: base ?? draft)
        draft = put.draft
        chips += put.chips.filter { chip in !chips.contains { $0.id == chip.id } }
        if let files = put.files {
            Task {
                do {
                    attach(
                        try await files.value.map {
                            var file = Pick.file($0.url)
                            file.stashId = $0.item.id
                            return file
                        })
                } catch {
                    problem = "Could not pick: \(Sikemux.problem(error))"
                }
            }
        }
        if !put.restored.isEmpty { Task { await StashStore.shared.take(put.restored) } }
    }

    private func flipWorktree(_ project: ProjectInfo, _ now: Bool) {
        guard let connection = Hub.shared.live(core).connection else { return }
        let on = !now
        flipped = (project.id, on)
        Task {
            do {
                _ = try await connection.call(
                    PhoneCall.SetWorktreeDefault.self, WorktreeDefaultParams(project: project.path, on: on))
            } catch {
                flipped = nil
                problem = "Could not change the worktree default: \(Sikemux.problem(error))"
            }
        }
    }

    /// Starts the agent, makes its worktree when asked, and leaves the message with the chat to go once it is attached.
    private func start(launcher: LauncherInfo?, project: ProjectInfo?, worktree: Bool) {
        let text = StashWords.withContext(
            draft.trimmingCharacters(in: .whitespacesAndNewlines), chips.map(StashWords.contextChip))
        guard sending == nil, !text.isEmpty || !attachments.isEmpty, let launcher, let project,
            let connection = Hub.shared.live(core).connection
        else { return }
        Haptics.tap()
        let since = Date()
        let shown = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        let message = ChatMessage(
            id: "starting", role: .user, parts: shown.isEmpty ? [] : [.text(.init(id: "starting-text", text: shown))],
            attachments: attachments.isEmpty ? nil : attachments.map(\.uri.absoluteString),
            sentAt: Int(since.timeIntervalSince1970 * 1000))
        context = TranscriptContext(
            provider: launcher.provider, core: core, agentId: nil, folds: Folds(), messages: { [message] },
            toolDetail: nil)
        sending = Sending(message: message, draft: draft, attachments: attachments, since: since)
        draft = ""
        attachments = []
        problem = nil
        Task {
            do {
                var agentId = started
                if agentId == nil {
                    let made = try await connection.startChat(
                        launcher: launcher.id, project: project.id, permissionMode: chosenMode?.rawValue)
                    agentId = made
                    started = made
                }
                guard let agentId else { return }
                if worktree && !inWorktree {
                    step = "Creating worktree"
                    let made = try await connection.call(
                        PhoneCall.CreateAgentWorktree.self, AgentWorktreeParams(agentId: agentId, message: text))
                    inWorktree = true
                    if let setupError = made.setupError {
                        fail("\(setupError). The worktree is kept at \(made.worktree.path); send again to start there.")
                        return
                    }
                    step = "Starting the agent in the worktree"
                }
                var files = sending?.attachments ?? []
                let (sent, failure) = await Uploads.each(
                    files,
                    put: { file in
                        let bytes = try await Task.detached { try Data(contentsOf: file.uri) }.value
                        return try await connection.attachFile(
                            agentId: agentId, name: file.name, mime: file.mime, bytes: bytes)
                    },
                    mark: { id, change in
                        if let at = files.firstIndex(where: { $0.id == id }) { change(&files[at]) }
                    })
                guard let sent else {
                    fail(failure ?? "Could not send every file to the host", files: files)
                    return
                }
                ChatSession.of(core: core, agentId: agentId).sendWhenAttached(text, sent)
                let taken = sent.compactMap(\.stashId) + chips.map(\.id)
                if !taken.isEmpty { Task { await StashStore.shared.take(taken) } }
                chips = []
                DraftStore.clear(keptKey)
                Router.shared.replace(.chat(core: core, agent: agentId))
            } catch {
                fail(Sikemux.problem(error))
            }
        }
    }

    private func fail(_ said: String, files: [Attachment]? = nil) {
        Haptics.failure()
        problem = said
        draft = sending?.draft ?? draft
        attachments = files ?? sending?.attachments ?? attachments
        sending = nil
        step = nil
    }
}

private struct AgentPicker: View {
    @Environment(\.palette) private var palette
    @Environment(\.hostCore) private var core
    let launchers: [LauncherInfo]
    let chosen: LauncherInfo?
    let onChoose: (LauncherInfo) -> Void

    var body: some View {
        var seen = Set<String>()
        let agents = launchers.filter { seen.insert($0.provider).inserted }
        VStack(spacing: 0) {
            SheetLabel("Agent")
            LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: 6), count: 4), spacing: 6) {
                ForEach(agents, id: \.provider) { launcher in
                    let on = launcher.provider == chosen?.provider
                    Button {
                        onChoose(on ? chosen ?? launcher : launcher)
                    } label: {
                        VStack(spacing: 6) {
                            AgentMark(provider: launcher.provider, size: 22)
                            Text(Catalog.label(launcher.provider, core: core))
                                .font(Theme.ui(12))
                                .foregroundStyle(on ? palette.ink : palette.tertiary)
                                .lineLimit(1)
                        }
                        .frame(maxWidth: .infinity, minHeight: 66)
                        .background(
                            RoundedRectangle(cornerRadius: 12, style: .continuous).fill(
                                on ? palette.active : palette.raised)
                        )
                        .overlay(
                            RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(
                                on ? palette.borderStrong : palette.border, lineWidth: 1))
                    }
                    .buttonStyle(.plain)
                    .accessibilityAddTraits(on ? [.isSelected] : [])
                }
            }
        }
    }
}

private struct AccountPicker: View {
    @Environment(\.palette) private var palette
    @Environment(\.hostCore) private var core
    let accounts: [LauncherInfo]
    let chosen: LauncherInfo
    let onChoose: (LauncherInfo) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 10) {
                AgentMark(provider: chosen.provider, size: 20)
                Text(Catalog.label(chosen.provider, core: core)).font(Theme.ui(17, .semibold)).tracking(-0.35)
                    .foregroundStyle(palette.ink)
            }
            .padding(.horizontal, 4)
            SheetLabel("Account")
            VStack(spacing: 0) {
                ForEach(Array(accounts.enumerated()), id: \.element.id) { index, launcher in
                    let on = launcher.id == chosen.id
                    Button {
                        onChoose(launcher)
                    } label: {
                        HStack(spacing: 12) {
                            AgentMark(provider: launcher.provider, size: 18)
                            Text(accountName(launcher, core: core))
                                .font(Theme.ui(15.5, .medium))
                                .foregroundStyle(on ? palette.ink : palette.secondary)
                                .lineLimit(1)
                                .frame(maxWidth: .infinity, alignment: .leading)
                            if on { IconView(icon: Icons.check, size: 17).foregroundStyle(palette.ink) }
                        }
                        .padding(.horizontal, 14)
                        .frame(minHeight: 52)
                        .background(on ? palette.active : .clear)
                        .overlay(alignment: .top) { if index > 0 { Rectangle().fill(palette.border).frame(height: 1) } }
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityAddTraits(on ? [.isSelected] : [])
                }
            }
            .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(palette.raised))
            .clipShape(RoundedRectangle(cornerRadius: 14, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(palette.border, lineWidth: 1))
        }
    }
}
