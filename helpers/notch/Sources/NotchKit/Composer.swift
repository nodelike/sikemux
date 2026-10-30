import AppKit
import SwiftUI

/// Sikemux's chat composer, for starting an agent from the notch: add files,
/// the yolo toggle, the model picker, effort, project, and send.
struct Composer: View {
    let store: NotchStore
    @Bindable var island: IslandModel
    @FocusState private var focused: Bool

    private var config: LauncherConfig { LauncherConfig(store.launcher(for: island.provider)) }
    private var project: ProjectInfo? { island.project ?? store.defaultProject }

    private var modelLabel: String {
        guard let model = config.model else { return "Default model" }
        let value = island.model ?? model.current
        return model.choices.first { $0.value == value }?.label ?? value
    }

    private var effortLabel: String? {
        guard let effort = config.effort else { return nil }
        let value = island.effort ?? effort.current
        return effort.choices.first { $0.value == value }?.label ?? value
    }

    private var canSend: Bool {
        project != nil && !island.draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            TextField("What should it do?", text: $island.draft, axis: .vertical)
                .textFieldStyle(.plain)
                .font(Theme.ui(13.5, .medium))
                .foregroundStyle(Theme.ink)
                .lineLimit(2...4)
                .frame(minHeight: 44, alignment: .topLeading)
                .padding(.horizontal, 6)
                .focused($focused)
                .onKeyPress(.return, phases: .down) { press in
                    if press.modifiers.contains(.shift) { return .ignored }
                    send()
                    return .handled
                }
            if !island.attachments.isEmpty {
                attachments.padding(.top, 6)
            }
            bar.padding(.top, 6)
            if let menu = island.menu {
                PickerMenu(store: store, island: island, menu: menu, config: config).padding(.top, 8)
            }
        }
        .padding(EdgeInsets(top: 12, leading: 8, bottom: 8, trailing: 8))
        .background(RoundedRectangle(cornerRadius: 16).fill(Theme.raised))
        .overlay(RoundedRectangle(cornerRadius: 16).stroke(focused ? Theme.accentLine : .clear, lineWidth: 1))
        .onAppear {
            DispatchQueue.main.async { focused = true }
        }
    }

    private var attachments: some View {
        HStack(spacing: 6) {
            ForEach(island.attachments, id: \.self) { path in
                HStack(spacing: 5) {
                    IconView(icon: Icons.file, size: 12).foregroundStyle(Theme.inkFaint)
                    Text((path as NSString).lastPathComponent).lineLimit(1)
                    Button {
                        island.attachments.removeAll { $0 == path }
                    } label: {
                        Text("×").foregroundStyle(Theme.inkFaint)
                    }
                    .buttonStyle(.plain)
                }
                .font(Theme.ui(11.5))
                .foregroundStyle(Theme.inkDim)
                .padding(.horizontal, 9)
                .frame(height: 24)
                .background(Capsule().fill(Theme.raised))
            }
        }
        .padding(.horizontal, 4)
    }

    private var bar: some View {
        HStack(spacing: 3) {
            round(Icons.plus, size: 17, help: "Add files") { chooseFiles() }
            YoloToggle(on: $island.yolo)
            trigger(.model, accent: true) {
                AgentMark(provider: island.provider, size: 17)
                Text(modelLabel).lineLimit(1)
            }
            if let effortLabel {
                trigger(.effort) { Text(effortLabel).lineLimit(1) }
            }
            trigger(.project) {
                IconView(icon: Icons.folder, size: 13)
                Text(project?.name ?? "Project").lineLimit(1)
            }
            Spacer(minLength: 0)
            Button(action: send) {
                IconView(icon: Icons.arrowUp, size: 15)
                    .foregroundStyle(Theme.onAccent)
                    .frame(width: 30, height: 30)
                    .background(Circle().fill(Theme.ink))
            }
            .buttonStyle(.plain)
            .opacity(canSend ? 1 : 0.28)
            .disabled(!canSend)
            .padding(.leading, 3)
            .help("Start agent")
        }
    }

    private func round(_ icon: IconDef, size: CGFloat, help: String, action: @escaping () -> Void) -> some View {
        HoverCapsule(action: action) {
            IconView(icon: icon, size: size).foregroundStyle(Theme.inkDim).frame(width: 31, height: 31)
        }
        .help(help)
    }

    private func trigger<Label: View>(_ menu: IslandModel.Menu, accent: Bool = false, @ViewBuilder label: @escaping () -> Label) -> some View {
        HoverCapsule(selected: island.menu == menu, selectedFill: Theme.hover) {
            island.menu = island.menu == menu ? nil : menu
        } label: {
            HStack(spacing: 6) {
                label()
                IconView(icon: Icons.chevron, size: 10).rotationEffect(.degrees(90)).opacity(0.6)
            }
            .font(Theme.ui(11.5))
            .foregroundStyle(accent ? Theme.accent : Theme.inkDim)
            .padding(.horizontal, 11)
            .frame(height: 31)
            .fixedSize()
        }
    }

    /// The chooser needs the helper in front; afterwards the person's app is put
    /// back in front and the prompt takes the keyboard again.
    private func chooseFiles() {
        let previous = NSWorkspace.shared.frontmostApplication
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = true
        panel.canChooseDirectories = false
        island.choosingFiles = true
        NSApp.activate(ignoringOtherApps: true)
        if panel.runModal() == .OK {
            island.attachments.append(contentsOf: panel.urls.map(\.path))
        }
        if let previous, previous != NSRunningApplication.current { previous.activate() }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) { [island] in
            island.choosingFiles = false
            if island.isOpen { island.keyboard?(.take) }
        }
    }

    private func send() {
        guard canSend, let project else { return }
        let launch = NotchStore.Launch(
            provider: island.provider,
            project: project,
            yolo: island.yolo,
            model: island.model,
            effort: island.effort,
            text: island.draft.trimmingCharacters(in: .whitespacesAndNewlines),
            paths: island.attachments
        )
        store.start(launch) { started in
            guard started else { return }
            island.draft = ""
            island.attachments = []
            island.menu = nil
            island.tab = .inbox
            island.keyboard?(.give)
        }
    }
}

/// The composer's toggle: a bolt and YOLO in the rainbow, or a shield and SAFE.
struct YoloToggle: View {
    @Binding var on: Bool

    var body: some View {
        HoverCapsule {
            on.toggle()
        } label: {
            HStack(spacing: 5) {
                IconView(icon: on ? Icons.shieldBolt : Icons.shield, size: 12)
                    .foregroundStyle(on ? Theme.accent : Theme.inkFaint)
                label
            }
            .padding(.horizontal, 11)
            .frame(height: 31)
        }
        .help(on ? "YOLO: the agent runs without asking" : "Safe: the agent asks before it acts")
    }

    @ViewBuilder
    private var label: some View {
        let text = Text(on ? "YOLO" : "SAFE").font(Theme.ui(10.5, .medium)).tracking(0.84)
        if on {
            text.foregroundStyle(Theme.yolo)
        } else {
            text.foregroundStyle(Theme.inkFaint)
        }
    }
}

/// The composer's picker, opening downward so the island grows to hold it.
struct PickerMenu: View {
    let store: NotchStore
    @Bindable var island: IslandModel
    let menu: IslandModel.Menu
    let config: LauncherConfig
    @State private var query = ""

    private struct Row: Identifiable {
        let id: String
        let label: String
        let selected: Bool
        let pick: () -> Void
    }

    private var rows: [Row] {
        let all: [Row]
        switch menu {
        case .model:
            let select = config.model
            let current = island.model ?? select?.current
            all = (select?.choices ?? []).map { choice in
                Row(id: choice.value, label: choice.label, selected: choice.value == current) { island.model = choice.value }
            }
        case .effort:
            let select = config.effort
            let current = island.effort ?? select?.current
            all = (select?.choices ?? []).map { choice in
                Row(id: choice.value, label: choice.label, selected: choice.value == current) { island.effort = choice.value }
            }
        case .project:
            let current = island.project ?? store.defaultProject
            all = store.view.workspace.projects.map { project in
                Row(id: project.id, label: project.name, selected: project == current) { island.project = project }
            }
        }
        guard !query.isEmpty else { return all }
        return all.filter { $0.label.localizedCaseInsensitiveContains(query) }
    }

    private var placeholder: String {
        switch menu {
        case .model: return "Search model…"
        case .effort: return "Search reasoning effort…"
        case .project: return "Search projects…"
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            TextField(placeholder, text: $query)
                .textFieldStyle(.plain)
                .font(Theme.ui(12.5, .medium))
                .foregroundStyle(Theme.ink)
                .padding(.horizontal, 12)
                .padding(.vertical, 10)
            Divider().overlay(Theme.hairline)
            if menu == .model {
                agentChips
                Divider().overlay(Theme.hairline)
            }
            ScrollView(.vertical, showsIndicators: false) {
                VStack(spacing: 0) {
                    if rows.isEmpty {
                        Text(menu == .model ? "Start this agent once in Sikemux to list its models" : "No matches")
                            .font(Theme.ui(11.5))
                            .foregroundStyle(Theme.inkFaint)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(.horizontal, 10)
                            .padding(.vertical, 8)
                    }
                    ForEach(rows) { row in
                        OptionRow(provider: menu == .model ? island.provider : nil, label: row.label, selected: row.selected, isProject: menu == .project) {
                            row.pick()
                            island.menu = nil
                        }
                    }
                }
                .padding(5)
            }
            .frame(maxHeight: 170)
            .fixedSize(horizontal: false, vertical: true)
        }
        .background(RoundedRectangle(cornerRadius: 12).fill(Color.black.opacity(0.5)))
        .clipShape(RoundedRectangle(cornerRadius: 12))
    }

    private var agentChips: some View {
        HStack(spacing: 4) {
            ForEach(store.providers, id: \.self) { provider in
                HoverCapsule(selected: island.provider == provider, selectedFill: Theme.accentSoft) {
                    island.provider = provider
                    island.model = nil
                    island.effort = nil
                } label: {
                    HStack(spacing: 6) {
                        AgentMark(provider: provider, size: 15)
                        Text(agentName(provider))
                    }
                    .font(Theme.ui(11.5))
                    .foregroundStyle(island.provider == provider ? Theme.ink : Theme.inkDim)
                    .padding(.horizontal, 9)
                    .frame(height: 26)
                }
            }
        }
        .padding(6)
    }
}

private struct OptionRow: View {
    let provider: String?
    let label: String
    let selected: Bool
    let isProject: Bool
    let pick: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: pick) {
            HStack(spacing: 9) {
                if let provider { AgentMark(provider: provider, size: 15) }
                if isProject { IconView(icon: Icons.folder, size: 13).foregroundStyle(Theme.inkDim) }
                Text(label).font(Theme.ui(12.5, .medium)).frame(maxWidth: .infinity, alignment: .leading)
                if selected { IconView(icon: Icons.check, size: 14) }
            }
            .foregroundStyle(selected ? Theme.accent : Theme.ink)
            .padding(.horizontal, 10)
            .padding(.vertical, 7)
            .background(RoundedRectangle(cornerRadius: 9).fill(hovering ? Theme.hover : .clear))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
    }
}
