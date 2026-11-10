import SwiftUI

/// One host: its agents, terminals and board, scoped to a project, with New chat for a phone with full access.
struct DeviceScreen: View {
    let core: String
    var linkedTab: Route.DeviceTab?

    var body: some View {
        DeviceContent(core: core, linkedTab: linkedTab).watching(core)
    }
}

private struct DeviceContent: View {
    @Environment(\.palette) private var palette
    let core: String
    let linkedTab: Route.DeviceTab?
    private let hub = Hub.shared

    @State private var tab: Route.DeviceTab = .agents
    @State private var picking = false
    @State private var options = false
    @State private var filter = "all"
    @State private var held: String?
    @State private var scrolled: CGFloat = 0
    @State private var headHeight: CGFloat = 0
    @State private var barHeight: CGFloat = 34

    private static let newChatHeight: CGFloat = 52
    /// How far rows take to fade out as they reach the tabs pinned at the top.
    private static let fade: CGFloat = 22

    var body: some View {
        let live = hub.live(core)
        let device = hub.device(core)
        let status = HostStatus(live, device: device)
        let view = live.view
        let hostName = device?.displayName ?? "Host"
        let away: Bool = {
            if case .closed = live.status { return true }
            return status.problem != nil
        }()

        Screen {
            Nav(back: "Devices", title: {
                if view != nil {
                    NavTitle(text: hostName).opacity(titleShown)
                }
            }, end: {
                if device != nil { IconButton(icon: Icons.more, label: "Options") { options = true } }
            })
            if view == nil || status.unpaired { HostHead(name: hostName, line: status.line, online: status.online) }
            if status.unpaired {
                Unpaired(device: device)
            } else if away && view == nil {
                unreachable(status, device)
            } else if let view {
                content(view, live: live, device: device, status: status, hostName: hostName)
            } else {
                Working().frame(maxWidth: .infinity, maxHeight: .infinity).padding(.bottom, 120)
            }
        }
        .sheet(shown: $options) {
            if let device { ForgetSheet(device: device) { options = false } }
        }
        .onAppear { follow(linkedTab) }
        .onChange(of: linkedTab) { _, next in follow(next) }
    }

    private func follow(_ link: Route.DeviceTab?) {
        if let link, link != .recent { tab = link }
    }

    private var titleShown: Double {
        Double(min(1, max(0, (scrolled - (headHeight - 48)) / 36)))
    }

    @ViewBuilder
    private func unreachable(_ status: HostStatus, _ device: PairedDevice?) -> some View {
        let words: (String, String?) = switch status.outdated {
        case .host: ("This host needs a newer Sikemux", nil)
        case .phone: ("Update this app", "This host runs a newer Sikemux.")
        case nil: ("Can't reach this host", "It may be asleep, offline, or have remote access turned off.")
        }
        Prompt(icon: status.outdated != nil ? Icons.download : (device?.kind ?? .laptop).icon, title: words.0, detail: words.1, fill: true) {
            if status.outdated == nil {
                HStack(spacing: 10) {
                    Working()
                    Text("Trying again").metaStyle(palette)
                }
                .padding(.top, 16)
                ActionButton("Try now") {
                    Haptics.tap()
                    hub.retry(core)
                }
                .padding(.top, 24)
            }
        }
    }

    @ViewBuilder
    private func content(_ view: DeviceView, live: HostLive, device: PairedDevice?, status: HostStatus, hostName: String) -> some View {
        let scope = view.workspace.projects.first { $0.id == device?.project }
        let asking = scope.map { view.needs(in: $0) } ?? view.inbox.items.filter { $0.tier == .needs }.count
        let providers = Self.providers(view, scope)
        let provider = providers.contains(filter) ? filter : "all"
        let filtering = tab == .agents && providers.count > 1
        let starts = device?.access == "full" && status.online && tab == .agents
        let pinned = headHeight > 0 && scrolled >= headHeight - barHeight - Self.fade
        let barTop = max(0, headHeight - scrolled)

        ZStack(alignment: .top) {
            ScrollView(showsIndicators: false) {
                VStack(spacing: 0) {
                    HostHead(name: hostName, line: status.line, online: status.online)
                        .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { headHeight = $0 }
                        .onGeometryChange(for: CGFloat.self) { $0.frame(in: .named("list")).minY } action: { scrolled = -$0 }
                    Color.clear.frame(height: barHeight)
                    Group {
                        switch tab {
                        case .terminals: Terminals(view: view, scope: scope)
                        case .board: BoardTab(core: core, live: live, view: view, scope: scope, onScope: scopeTo)
                        default:
                            DeviceAgents(core: core, view: view, scope: scope, provider: provider, full: device?.access == "full",
                                         hostName: hostName, onTab: { tab = $0 },
                                         onHold: device?.access == "full" ? { held = $0 } : nil)
                        }
                    }
                    .padding(.horizontal, 16)
                }
                .padding(.bottom, bottomGap + (starts ? Self.newChatHeight + 24 : 24))
            }
            .coordinateSpace(.named("list"))
            .pausesBackdrop()
            .opacity(status.online ? 1 : 0.5)
            .accessibilityHint(status.online ? "" : "Out of date until the host answers again")
            .mask {
                VStack(spacing: 0) {
                    if pinned {
                        Color.clear.frame(height: barHeight)
                        LinearGradient(colors: [.clear, .black], startPoint: .top, endPoint: .bottom).frame(height: Self.fade)
                    }
                    Color.black
                }
            }

            bar(view: view, scope: scope, asking: asking, filtering: filtering, provider: provider)
                .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { barHeight = $0 }
                .offset(y: barTop)
        }
        .frame(maxHeight: .infinity)
        .overlay(alignment: .bottomTrailing) {
            if starts {
                Button {
                    Router.shared.push(.newChat(core: core, project: scope?.id))
                } label: {
                    HStack(spacing: 8) {
                        IconView(icon: Icons.plus, size: 18)
                        Text("New chat").font(Theme.ui(15, .semibold))
                    }
                    .foregroundStyle(palette.ground)
                    .padding(.leading, 16)
                    .padding(.trailing, 20)
                    .frame(height: Self.newChatHeight)
                    .background(Capsule().fill(palette.ink).shadow(color: .black.opacity(0.45), radius: 18, y: 8))
                }
                .buttonStyle(PressFade())
                .padding(.trailing, 16)
                .padding(.bottom, bottomGap)
                .accessibilityLabel("New chat")
            }
        }
        .sheet(shown: Binding(get: { held != nil }, set: { if !$0 { held = nil } })) {
            if let shown = view.inbox.agents.first(where: { $0.agentId == held }) {
                AwaySheet(core: core, shown: shown) { held = nil }
            }
        }
        .sheet(shown: $picking, tall: view.workspace.projects.count > 6) {
            ProjectSheet(
                device: device?.displayName, projects: view.workspace.projects, chosen: scope?.id,
                all: HostStatus.summary(view),
                tail: { ProjectTail(view: view, project: $0) },
                providers: filtering ? ProviderFilter(offered: providers, chosen: provider) { filter = $0 } : nil,
                onChoose: scopeTo)
        }
    }

    private func bar(view: DeviceView, scope: ProjectInfo?, asking: Int, filtering: Bool, provider: String) -> some View {
        HStack(spacing: 12) {
            HStack(spacing: 20) {
                HostTab(label: "Agents", count: asking, on: tab == .agents) { switchTo(.agents) }
                HostTab(label: "Terminals", on: tab == .terminals) { switchTo(.terminals) }
                HostTab(label: "Board", on: tab == .board) { switchTo(.board) }
            }
            Spacer(minLength: 0)
            if !view.workspace.projects.isEmpty || filtering {
                Button { picking = true } label: {
                    HStack(spacing: 6) {
                        IconView(icon: Icons.folder, size: 14).foregroundStyle(palette.live)
                        Text(scope?.name ?? "All projects")
                            .font(Theme.ui(13))
                            .foregroundStyle(palette.secondary)
                            .lineLimit(1)
                        if filtering, provider != "all" { AgentMark(provider: provider, size: 14) }
                        IconView(icon: Icons.chevron, size: 12)
                            .foregroundStyle(palette.inkFaint)
                            .rotationEffect(.degrees(90))
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(PressFade())
                .accessibilityLabel("Project")
            }
        }
        .padding(.horizontal, 18)
        .padding(.top, 6)
        .padding(.bottom, 8)
    }

    private func switchTo(_ next: Route.DeviceTab) {
        if next != tab { Haptics.select() }
        tab = next
    }

    private func scopeTo(_ project: String?) {
        picking = false
        Task { await hub.update(core) { $0.project = project } }
    }

    private static func providers(_ view: DeviceView, _ scope: ProjectInfo?) -> [String] {
        var seen: [String] = []
        let chats = view.chats.filter { scope == nil || view.projectOf(agent: $0.agentId) == scope?.id }.map(\.provider)
        let recent = view.recent.filter { scope == nil || $0.project == scope?.id }.map(\.provider)
        for provider in chats + recent where !seen.contains(provider) { seen.append(provider) }
        return seen
    }
}

private struct HostHead: View {
    @Environment(\.palette) private var palette
    let name: String
    let line: String
    let online: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 8) {
                if online {
                    Circle().fill(palette.live).frame(width: 7, height: 7).shadow(color: palette.live, radius: 2.5)
                } else {
                    Circle().strokeBorder(palette.rest, lineWidth: 1.5).frame(width: 7, height: 7)
                }
                Text(line).font(Theme.ui(13)).foregroundStyle(palette.ink.opacity(0.72)).lineLimit(1)
            }
            Text(name)
                .font(Theme.ui(32, .semibold))
                .tracking(-1.12)
                .foregroundStyle(palette.ink)
                .lineLimit(1)
                .shadow(color: palette.isLight ? .clear : .black.opacity(0.65), radius: 9, y: 2)
                .accessibilityAddTraits(.isHeader)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 18)
        .padding(.top, 10)
        .padding(.bottom, 16)
    }
}

private struct HostTab: View {
    @Environment(\.palette) private var palette
    let label: String
    var count = 0
    let on: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 7) {
                Text(label)
                    .font(Theme.ui(15, .semibold))
                    .tracking(-0.15)
                    .foregroundStyle(on ? palette.ink : palette.tertiary)
                if count > 0 {
                    Text("\(count)")
                        .font(Theme.ui(11, .semibold))
                        .foregroundStyle(palette.ground)
                        .padding(.horizontal, 6)
                        .frame(minWidth: 16, minHeight: 16)
                        .background(Capsule().fill(palette.ink))
                }
            }
            .padding(.vertical, 6)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(on ? [.isSelected, .isButton] : .isButton)
    }
}

/// What runs in a project, as its row in the project sheet shows it.
private struct ProjectTail: View {
    @Environment(\.palette) private var palette
    let view: DeviceView
    let project: ProjectInfo

    var body: some View {
        let chats = view.chats.filter { view.projectOf(agent: $0.agentId) == project.id }
        let terminals = view.sessions.filter { $0.project == project.id && $0.running }.count
        if !chats.isEmpty || terminals > 0 {
            HStack(spacing: 8) {
                if view.needs(in: project) > 0 { NeedsYou() }
                if !chats.isEmpty {
                    HStack(spacing: -4) {
                        ForEach(chats.prefix(3), id: \.agentId) { chat in
                            AgentFaceView(agentId: chat.agentId, provider: chat.provider, group: chat.group, size: 16)
                                .frame(width: 16, height: 16)
                        }
                    }
                } else {
                    HStack(spacing: 4) {
                        IconView(icon: Icons.command, size: 13).foregroundStyle(palette.tertiary)
                        Text("\(terminals)").metaStyle(palette)
                    }
                }
            }
        }
    }
}

/// The host turned this phone away: only forgetting it here and connecting again helps.
private struct Unpaired: View {
    @Environment(\.palette) private var palette
    let device: PairedDevice?
    @State private var forget = Forgetting()

    var body: some View {
        Prompt(icon: Icons.unlink, title: "This host no longer knows this phone", fill: true) {
            ActionButton(forget.busy ? "Forgetting…" : "Forget this host", kind: .danger, icon: Icons.trash) {
                if let device { Task { await forget.leave(device) } }
            }
            .disabled(forget.busy || device == nil)
            .padding(.top, 24)
        }
    }
}
