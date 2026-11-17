import AppKit
import SwiftUI
import UniformTypeIdentifiers

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
            HoverCapsule(selected: island.tab == .inbox) {
                select(.inbox)
            } label: {
                HStack(spacing: 6) {
                    Text("Agents").foregroundStyle(island.tab == .inbox ? Theme.ink : Theme.inkDim)
                    Text("\(store.entries.count)").foregroundStyle(island.tab == .inbox ? Theme.inkDim : Theme.inkFaint).monospacedDigit()
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
            if let agentId = store.entries.lazy.compactMap(\.agentId).first { store.focus(agentId) }
        } label: {
            IconView(icon: Icons.brand, size: 14)
                .foregroundStyle(Theme.ink)
                .frame(width: 26, height: 26)
        }
        .help(store.options.dev ? "Open Sikemux Dev" : "Open Sikemux")
    }
}

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
            case .inbox: AgentList(store: store, island: island, marks: marks)
            case .compose: Composer(store: store, island: island)
            }
        }
        .padding(.top, 6)
    }
}

/// The open agents in the app rail's order, in one list.
struct AgentList: View {
    let store: NotchStore
    let island: IslandModel
    let marks: Namespace.ID

    /// The open island's width inside its inset, as IslandView lays it out.
    private var listWidth: CGFloat { (island.geometry.hasNotch ? 680 : 560) - 2 * (Radii.open.top + 12) }

    /// How tall the list grows: a few rows, or with a call's card open in it, the screen below the island's own
    /// header, so the card and its Go with this fit whole.
    private var room: CGFloat {
        guard !store.calls.isEmpty else { return Self.rows }
        return max(Self.rows, island.canvas.height - island.geometry.height - Self.chrome)
    }

    private static let rows: CGFloat = 330
    /// The island's header, its padding and its shadow, above and below the list.
    private static let chrome: CGFloat = 110

    var body: some View {
        if store.entries.isEmpty && store.view.hostAlerts.isEmpty && store.view.busy == nil {
            VStack(spacing: 14) {
                Text("No agents open").font(Theme.ui(13)).foregroundStyle(Theme.inkDim)
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
            ScrollViewReader { scroller in
            ScrollView(.vertical, showsIndicators: false) {
                VStack(alignment: .leading, spacing: 0) {
                    CallsInList(store: store, island: island, width: listWidth)
                    if let busy = store.view.busy {
                        BusyRow(store: store, busy: busy)
                    }
                    ForEach(store.view.hostAlerts) { alert in
                        HostAlertRow(store: store, entry: alert)
                    }
                    ForEach(store.entries) { entry in
                        if entry.mark == .needs, entry.ask != nil {
                            AskCard(store: store, island: island, entry: entry, marks: marks)
                        } else {
                            EntryRow(store: store, entry: entry, marks: marks)
                        }
                    }
                }
                .background(GeometryReader { proxy in
                    Color.clear.onAppear { island.listScrolls = proxy.size.height > room }
                        .onChange(of: proxy.size.height) { _, height in island.listScrolls = height > room }
                })
            }
            .frame(maxHeight: room)
            .fixedSize(horizontal: false, vertical: true)
            .onAppear { scroller.scrollTo(CallsInList.footer, anchor: .bottom) }
            .onChange(of: island.listCall) { _, _ in scroller.scrollTo(CallsInList.footer, anchor: .bottom) }
            }
        }
    }
}

struct EntryRow: View {
    let store: NotchStore
    let entry: Entry
    let marks: Namespace.ID
    @State private var hovering = false

    var body: some View {
        Button {
            store.open(entry)
        } label: {
            RowLine(entry: entry, marks: marks)
                .padding(.horizontal, 12)
                .frame(height: 36)
                .background(RoundedRectangle(cornerRadius: 12).fill(hovering ? Theme.hover : .clear))
                .modifier(CalledWave(calledAt: entry.calledAt))
                .contentShape(RoundedRectangle(cornerRadius: 12))
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
    }
}

/// A host in trouble: the server in amber, or red once it is serious, what is wrong with how long, then the host.
struct HostAlertRow: View {
    let store: NotchStore
    let entry: Entry
    @State private var hovering = false

    var body: some View {
        Button {
            store.open(entry)
        } label: {
            HStack(spacing: 10) {
                IconView(icon: Icons.server, size: 16)
                    .foregroundStyle(entry.item?.hostAlert?.level == .danger ? Theme.danger : Theme.warn)
                    .frame(width: 20, height: 20)
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text(entry.item?.what ?? "").font(Theme.ui(13, .medium)).foregroundStyle(Theme.ink).lineLimit(1)
                    SinceText(mark: .needs, since: entry.since).fixedSize()
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                Text(entry.title).font(Theme.ui(11.5)).foregroundStyle(Theme.inkDim).lineLimit(1)
                    .frame(maxWidth: 150, alignment: .trailing).fixedSize()
            }
            .padding(.horizontal, 12)
            .frame(height: 36)
            .background(RoundedRectangle(cornerRadius: 12).fill(hovering ? Theme.hover : .clear))
            .contentShape(RoundedRectangle(cornerRadius: 12))
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
    }
}

/// This Mac overloaded by agents' work: the server in amber, Mac busy, then the load and the builds.
struct BusyRow: View {
    let store: NotchStore
    let busy: String
    @State private var hovering = false

    var body: some View {
        Button {
            store.openApp()
        } label: {
            HStack(spacing: 10) {
                IconView(icon: Icons.server, size: 16)
                    .foregroundStyle(Theme.warn)
                    .frame(width: 20, height: 20)
                Text("Mac busy").font(Theme.ui(13, .medium)).foregroundStyle(Theme.ink).lineLimit(1)
                    .frame(maxWidth: .infinity, alignment: .leading)
                Text(busy).font(Theme.ui(11.5)).foregroundStyle(Theme.inkDim).lineLimit(1).fixedSize()
            }
            .padding(.horizontal, 12)
            .frame(height: 36)
            .background(RoundedRectangle(cornerRadius: 12).fill(hovering ? Theme.hover : .clear))
            .contentShape(RoundedRectangle(cornerRadius: 12))
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
    }
}

/// A band of light that passes once over a row each time its agent is named with `@` or messaged by another agent, as on the app's rail.
struct CalledWave: ViewModifier {
    let calledAt: Int?
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var through: CGFloat = 1

    func body(content: Content) -> some View {
        content.overlay {
            GeometryReader { proxy in
                LinearGradient(
                    colors: [Theme.ink.opacity(0), Theme.ink.opacity(0.09), Theme.ink.opacity(0)],
                    startPoint: .leading,
                    endPoint: .trailing
                )
                .frame(width: proxy.size.width * 0.6)
                .offset(x: proxy.size.width * (through * 1.6 - 0.6))
            }
            .clipShape(RoundedRectangle(cornerRadius: 12))
            .allowsHitTesting(false)
        }
        .onChange(of: calledAt) { _, _ in
            guard !reduceMotion else { return }
            through = 0
            DispatchQueue.main.async {
                withAnimation(.timingCurve(0.3, 0, 0.2, 1, duration: 0.7)) { through = 1 }
            }
        }
    }
}

/// Laid out as the app's tab rail row: mark, task with how long ago, project, then the name, counts and state together at the end.
struct RowLine: View {
    let entry: Entry
    let marks: Namespace.ID

    var body: some View {
        HStack(spacing: 10) {
            Group {
                if entry.isChief {
                    IconView(icon: Icons.brand, size: 15).foregroundStyle(Theme.accent).frame(width: 20, height: 20)
                } else {
                    AgentLook(entry: entry, size: 20)
                }
            }
            .matchedGeometryEffect(id: entry.id, in: marks)
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                if entry.isChief {
                    Text("@Chief").font(Theme.ui(13, .medium)).foregroundStyle(Theme.ink).lineLimit(1).fixedSize()
                    Text(entry.agent?.titled == true ? entry.title : "Chief of staff")
                        .font(Theme.ui(13)).foregroundStyle(Theme.inkFaint).lineLimit(1)
                } else {
                    Text(entry.title).font(Theme.ui(13, .medium)).foregroundStyle(Theme.ink).lineLimit(1)
                }
                if entry.mark == .working, let attempt = entry.reconnecting {
                    Text(attempt > 0 ? "Reconnecting… \(attempt)" : "Reconnecting…")
                        .font(Theme.ui(11.5))
                        .foregroundStyle(Theme.inkFaint)
                        .fixedSize()
                } else if entry.since != nil {
                    SinceText(mark: entry.mark, since: entry.since).fixedSize()
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            ProjectLabel(project: entry.project).frame(maxWidth: 150, alignment: .trailing).fixedSize()
            if entry.name != nil || entry.subagents > 0 || entry.tasks > 0 || entry.inLine != nil || entry.mark != .idle {
                HStack(spacing: 8) {
                    if let name = entry.name, !entry.isChief {
                        Text("@\(name)").font(Theme.ui(11)).foregroundStyle(Theme.inkFaint).lineLimit(1).fixedSize()
                    }
                    if entry.subagents > 0 { SubagentCount(count: entry.subagents) }
                    if entry.tasks > 0 { ShellCount(count: entry.tasks) }
                    if let line = entry.inLine { LinePlace(line: line) }
                    TurnMark(mark: entry.mark, turn: entry.agent?.turn)
                }
            }
        }
    }
}

struct AskCard: View {
    let store: NotchStore
    let island: IslandModel
    let entry: Entry
    let marks: Namespace.ID

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Button {
                store.open(entry)
            } label: {
                RowLine(entry: entry, marks: marks).contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            AskDetail(entry: entry, darker: true).padding(.top, 10)
            AnswerButtons(store: store, island: island, entry: entry, height: 28).padding(.top, 10)
        }
        .padding(.horizontal, 12)
        .padding(.top, 10)
        .padding(.bottom, 12)
        .background(RoundedRectangle(cornerRadius: 14).fill(Theme.raised))
        .padding(.bottom, 2)
    }
}

/// Dropping a file: start an agent with it, answer the question waiting with it, or hand it to
/// the agent that worked last.
struct DropTargets: View {
    let store: NotchStore
    let island: IslandModel

    private var asking: Entry? { store.entries.first { $0.question != nil } }

    private var recent: Entry? {
        store.entries.first { $0.isChat && ($0.mark == .working || $0.mark == .needs) } ?? store.entries.first(where: \.isChat)
    }

    var body: some View {
        HStack(spacing: 8) {
            target("new") {
                IconView(icon: Icons.plus, size: 20).foregroundStyle(Theme.ink)
                Text("New agent with this file").font(Theme.ui(13, .semibold)).foregroundStyle(Theme.ink)
                Text("\(agentName(island.provider)) in \(island.project?.name ?? store.defaultProject?.name ?? "a project")")
                    .font(Theme.ui(11.5)).foregroundStyle(Theme.inkDim)
            }
            if let asking, let question = asking.question {
                target(Self.answering) {
                    AgentLook(entry: asking, size: 20)
                    Text("Answer \(agentName(asking.provider))").font(Theme.ui(13, .semibold)).foregroundStyle(Theme.ink)
                    Text(question.text).font(Theme.ui(11.5)).foregroundStyle(Theme.inkDim).lineLimit(1)
                }
            }
            if let recent, recent.id != asking?.id {
                target(recent.id) {
                    AgentLook(entry: recent, size: 20)
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

    private static let answering = "answer"

    private func drop(_ paths: [String], on id: String) {
        guard !paths.isEmpty else { return }
        if id == Self.answering, let question = asking?.question {
            store.answer(question, paths: paths)
            island.set(.closed)
        } else if id == "new" {
            island.attachments = paths
            island.tab = .compose
            island.set(.open)
            island.keyboard?(.take)
        } else if let entry = store.entries.first(where: { $0.id == id }) {
            store.send(paths, to: entry)
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
