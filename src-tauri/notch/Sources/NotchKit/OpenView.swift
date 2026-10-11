import AppKit
import SwiftUI
import UniformTypeIdentifiers

/// The open island's band: tabs left of the camera, Sikemux right of it.
struct OpenHeader: View {
    let store: NotchStore
    @Bindable var island: IslandModel

    var body: some View {
        HeaderBand(geometry: island.geometry) {
            tabs
        } trailing: {
            tools
        }
    }

    private var tabs: some View {
        HStack(spacing: 4) {
            HoverCapsule(selected: island.tab == .agents) {
                select(.agents)
            } label: {
                HStack(spacing: 6) {
                    Text("Agents").foregroundStyle(island.tab == .agents ? Theme.ink : Theme.inkDim)
                    Text("\(store.agents.count)").foregroundStyle(island.tab == .agents ? Theme.inkDim : Theme.inkFaint).monospacedDigit()
                }
                .font(Theme.ui(12, .semibold))
                .padding(.horizontal, 11)
                .frame(height: 26)
            }
            HoverCapsule(selected: island.tab == .compose) {
                select(.compose)
            } label: {
                HStack(spacing: 6) {
                    IconView(icon: Icons.plus, size: 12)
                    Text("New agent")
                }
                .font(Theme.ui(12, .semibold))
                .foregroundStyle(island.tab == .compose ? Theme.ink : Theme.inkDim)
                .padding(.horizontal, 11)
                .frame(height: 26)
            }
        }
    }

    private func select(_ tab: IslandModel.Tab) {
        island.tab = tab
        island.menu = nil
        island.keyboard?(tab == .compose ? .take : .give)
    }

    @ViewBuilder
    private var tools: some View {
        if store.options.dev {
            Text("DEV")
                .font(Theme.mono(9.5, .semibold))
                .tracking(0.8)
                .foregroundStyle(Theme.accent)
                .padding(.horizontal, 7)
                .frame(height: 18)
                .background(Capsule().fill(Theme.accentSoft))
        }
        HoverCapsule {
            store.openApp()
            if let first = store.agents.first { store.focus(first.id) }
        } label: {
            IconView(icon: Icons.brand, size: 14)
                .foregroundStyle(Theme.ink)
                .frame(width: 26, height: 26)
        }
        .help(store.options.dev ? "Open Sikemux Dev" : "Open Sikemux")
    }
}

/// The open island below its band: the agents, or the composer.
struct OpenBody: View {
    let store: NotchStore
    @Bindable var island: IslandModel
    let marks: Namespace.ID

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            if let error = store.error {
                Text(error).font(Theme.ui(11.5)).foregroundStyle(Theme.danger).padding(.horizontal, 12).padding(.top, 4)
            }
            switch island.tab {
            case .agents: AgentList(store: store, island: island, marks: marks)
            case .compose: Composer(store: store, island: island)
            }
        }
        .padding(.top, 6)
    }
}

/// Agents grouped by what they need from the person.
struct AgentList: View {
    let store: NotchStore
    let island: IslandModel
    let marks: Namespace.ID

    var body: some View {
        if store.agents.isEmpty {
            VStack(spacing: 14) {
                Text("No agents running").font(Theme.ui(13)).foregroundStyle(Theme.inkDim)
                Button {
                    island.tab = .compose
                    island.keyboard?(.take)
                } label: {
                    HStack(spacing: 6) {
                        IconView(icon: Icons.plus, size: 12)
                        Text("New agent").font(Theme.ui(12.5, .semibold))
                    }
                    .foregroundStyle(Theme.onAccent)
                    .padding(.horizontal, 16)
                    .frame(height: 30)
                    .background(Capsule().fill(Theme.accent))
                }
                .buttonStyle(.plain)
            }
            .frame(maxWidth: .infinity)
            .padding(.vertical, 18)
            .onAppear { island.listScrolls = false }
        } else {
            ScrollView(.vertical, showsIndicators: false) {
                VStack(alignment: .leading, spacing: 0) {
                    group("Needs you", .blocked)
                    group("Working", .working)
                    group("Done", .done)
                    group("Idle", .idle)
                }
                .background(GeometryReader { proxy in
                    Color.clear.onAppear { island.listScrolls = proxy.size.height > 330 }
                        .onChange(of: proxy.size.height) { _, height in island.listScrolls = height > 330 }
                })
            }
            .frame(maxHeight: 330)
            .fixedSize(horizontal: false, vertical: true)
        }
    }

    @ViewBuilder
    private func group(_ label: String, _ state: AgentState) -> some View {
        let agents = store.agents.filter { $0.state == state }
        if !agents.isEmpty {
            Text(label)
                .font(Theme.ui(11, .semibold))
                .foregroundStyle(Theme.inkFaint)
                .padding(.horizontal, 12)
                .padding(.top, 10)
                .padding(.bottom, 4)
            ForEach(agents) { agent in
                if state == .blocked {
                    AskCard(store: store, island: island, agent: agent, marks: marks)
                } else {
                    AgentRow(store: store, agent: agent, marks: marks)
                }
            }
        }
    }
}

struct AgentRow: View {
    let store: NotchStore
    let agent: AgentItem
    let marks: Namespace.ID
    @State private var hovering = false

    var body: some View {
        Button {
            store.focus(agent.id)
        } label: {
            RowLine(store: store, agent: agent, marks: marks)
                .padding(.horizontal, 12)
                .frame(height: 36)
                .background(RoundedRectangle(cornerRadius: 12).fill(hovering ? Theme.hover : .clear))
                .contentShape(RoundedRectangle(cornerRadius: 12))
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
    }
}

/// Mark, task, project, time, state: one line.
struct RowLine: View {
    let store: NotchStore
    let agent: AgentItem
    let marks: Namespace.ID

    var body: some View {
        HStack(spacing: 10) {
            AgentMark(provider: agent.provider, size: 20).matchedGeometryEffect(id: agent.id, in: marks)
            Text(agent.title)
                .font(Theme.ui(13, .semibold))
                .foregroundStyle(Theme.ink)
                .lineLimit(1)
                .frame(maxWidth: .infinity, alignment: .leading)
            if agent.subagents > 0 { SubagentCount(count: agent.subagents) }
            ProjectLabel(project: agent.project).frame(maxWidth: 150, alignment: .trailing).fixedSize()
            if agent.state != .idle {
                SinceText(state: agent.state, since: store.since[agent.id]).frame(minWidth: 44, alignment: .trailing)
                StateMark(state: agent.state)
            } else if agent.background {
                StillRunning()
            }
        }
    }
}

struct AskCard: View {
    let store: NotchStore
    let island: IslandModel
    let agent: AgentItem
    let marks: Namespace.ID

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Button {
                store.focus(agent.id)
            } label: {
                RowLine(store: store, agent: agent, marks: marks).contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            if let ask = agent.ask {
                CommandText(ask: ask, darker: true).padding(.top, 10)
            }
            AnswerButtons(store: store, island: island, agent: agent, height: 28).padding(.top, 10)
        }
        .padding(.horizontal, 12)
        .padding(.top, 10)
        .padding(.bottom, 12)
        .background(RoundedRectangle(cornerRadius: 14).fill(Theme.raised))
        .padding(.bottom, 2)
    }
}

/// Dropping a file: start an agent with it, or hand it to the agent that worked last.
struct DropTargets: View {
    let store: NotchStore
    let island: IslandModel

    private var recent: AgentItem? {
        store.agents.first { $0.isChat && ($0.state == .working || $0.state == .blocked) } ?? store.agents.first { $0.isChat }
    }

    var body: some View {
        HStack(spacing: 8) {
            target("new") {
                IconView(icon: Icons.plus, size: 20).foregroundStyle(Theme.ink)
                Text("New agent with this file").font(Theme.ui(13, .semibold)).foregroundStyle(Theme.ink)
                Text("\(agentName(island.provider)) in \(island.project?.name ?? store.defaultProject?.name ?? "a project")")
                    .font(Theme.ui(11.5)).foregroundStyle(Theme.inkDim)
            }
            if let recent {
                target(recent.id) {
                    AgentMark(provider: recent.provider, size: 20)
                    Text("Send to \(agentName(recent.provider))").font(Theme.ui(13, .semibold)).foregroundStyle(Theme.ink)
                    Text(recent.title).font(Theme.ui(11.5)).foregroundStyle(Theme.inkDim).lineLimit(1)
                }
            }
        }
        .padding(.top, 6)
    }

    private func target<Label: View>(_ id: String, @ViewBuilder label: () -> Label) -> some View {
        let hot = island.dropTarget == id
        return VStack(spacing: 7) { label() }
            .padding(.horizontal, 12)
            .frame(maxWidth: .infinity)
            .frame(height: 104)
            .background(RoundedRectangle(cornerRadius: 14).fill(hot ? Theme.accentSoft : Color.white.opacity(0.05)))
            .overlay(RoundedRectangle(cornerRadius: 14).stroke(hot ? Theme.accentLine : .clear, lineWidth: 1))
            .onDrop(of: [.fileURL], isTargeted: Binding(
                get: { island.dropTarget == id },
                set: { targeted in
                    if targeted, island.dropTarget != id { Haptics.tick() }
                    island.dropTarget = targeted ? id : (island.dropTarget == id ? nil : island.dropTarget)
                }
            )) { providers in
                load(providers) { paths in drop(paths, on: id) }
                return true
            }
    }

    private func drop(_ paths: [String], on id: String) {
        guard !paths.isEmpty else { return }
        if id == "new" {
            island.attachments = paths
            island.tab = .compose
            island.set(.open)
            island.keyboard?(.take)
        } else if let agent = store.agents.first(where: { $0.id == id }) {
            store.send(paths, to: agent)
            island.set(.closed)
        }
    }

    private func load(_ providers: [NSItemProvider], done: @escaping ([String]) -> Void) {
        let collected = Collected()
        let group = DispatchGroup()
        for provider in providers {
            group.enter()
            _ = provider.loadObject(ofClass: URL.self) { url, _ in
                if let url { collected.add(url.path) }
                group.leave()
            }
        }
        group.notify(queue: .main) { done(collected.paths) }
    }
}

private final class Collected: @unchecked Sendable {
    private let lock = NSLock()
    private var found: [String] = []

    func add(_ path: String) {
        lock.lock()
        found.append(path)
        lock.unlock()
    }

    var paths: [String] {
        lock.lock()
        defer { lock.unlock() }
        return found
    }
}
