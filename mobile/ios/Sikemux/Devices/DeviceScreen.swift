import SwiftUI

/// One host: its agents, terminals and the calls it puts to the person, scoped to a project, with New chat for a phone with full access.
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
    @State private var followedLink = false
    @State private var picking = false
    @State private var options = false
    @State private var filter: String?
    @State private var held: String?
    @State private var scrolled: CGFloat = 0
    @State private var headHeight: CGFloat = 0
    @State private var barHeight: CGFloat = 34

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
            Nav(
                title: {
                    HostTitle(
                        name: hostName, online: status.online,
                        project: view.map { projectLabel($0, device: device) }, shown: titleShown,
                        onPick: view.flatMap { pickable($0, device: device) ? { picking = true } : nil })
                },
                end: {
                    if let view, !status.unpaired { navActions(view, device: device, status: status) }
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
        .onAppear {
            // Coming back from a chat shows this screen again; the tab switched to then stays.
            guard !followedLink else { return }
            followedLink = true
            follow(linkedTab)
        }
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
        let words: (String, String?) =
            switch status.outdated {
            case .host: ("This host needs a newer Sikemux", nil)
            case .phone: ("Update this app", "This host runs a newer Sikemux.")
            case nil: ("Can't reach this host", "It may be asleep, offline, or have remote access turned off.")
            }
        Prompt(
            icon: status.outdated != nil ? Icons.download : (device?.kind ?? .laptop).icon, title: words.0,
            detail: words.1, fill: true
        ) {
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
    private func content(
        _ view: DeviceView, live: HostLive, device: PairedDevice?, status: HostStatus, hostName: String
    ) -> some View {
        let scope = view.project(id: device?.project)
        let calling = scope.map { $0.calls ?? 0 } ?? HostCalls.of(core).open(in: nil).count
        let providers = Self.providers(view, scope)
        let provider = filter.flatMap { providers.contains($0) ? $0 : nil }
        let filtering = tab == .agents && providers.count > 1
        let pinned = headHeight > 0 && scrolled >= headHeight - barHeight - Self.fade
        let barTop = max(0, headHeight - scrolled)

        ZStack(alignment: .top) {
            ScrollView(showsIndicators: false) {
                VStack(spacing: 0) {
                    ProjectHead(
                        title: projectLabel(view, device: device), line: status.facts,
                        provider: filtering ? provider : nil,
                        onPick: pickable(view, device: device) ? { picking = true } : nil
                    )
                    .onGeometryChange(for: CGFloat.self) {
                        $0.size.height
                    } action: {
                        headHeight = $0
                    }
                    .onGeometryChange(for: CGFloat.self) {
                        $0.frame(in: .named("list")).minY
                    } action: {
                        scrolled = -$0
                    }
                    Color.clear.frame(height: barHeight)
                    Group {
                        switch tab {
                        case .terminals: Terminals(view: view, scope: scope)
                        case .calls:
                            CallsTab(
                                core: core, view: view, scope: scope,
                                canAnswer: device?.access != .watch && live.isOpen)
                        default:
                            DeviceAgents(
                                core: core, view: view, scope: scope, provider: provider,
                                full: device?.access == .full,
                                hostName: hostName, onTab: { tab = $0 },
                                onHold: device?.access == .full ? { held = $0 } : nil)
                        }
                    }
                    .padding(.horizontal, 16)
                }
                .padding(.bottom, bottomGap + 24)
            }
            .coordinateSpace(.named("list"))
            .pausesBackdrop()
            .opacity(status.online ? 1 : 0.5)
            .accessibilityHint(status.online ? "" : "Out of date until the host answers again")
            .mask {
                VStack(spacing: 0) {
                    if pinned {
                        Color.clear.frame(height: barHeight)
                        LinearGradient(colors: [.clear, .black], startPoint: .top, endPoint: .bottom).frame(
                            height: Self.fade)
                    }
                    Color.black
                }
            }

            HostTabs(tab: tab, calling: calling, onSwitch: switchTo)
                .onGeometryChange(for: CGFloat.self) {
                    $0.size.height
                } action: {
                    barHeight = $0
                }
                .offset(y: barTop)
        }
        .frame(maxHeight: .infinity)
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

    /// What the big title says: the project in view, or All projects.
    private func projectLabel(_ view: DeviceView, device: PairedDevice?) -> String {
        view.project(id: device?.project)?.name ?? "All projects"
    }

    /// The title opens the project picker when there is something to pick.
    private func pickable(_ view: DeviceView, device: PairedDevice?) -> Bool {
        !view.workspace.projects.isEmpty || Self.providers(view, view.project(id: device?.project)).count > 1
    }

    /// New chat, in the bar beside Options.
    @ViewBuilder
    private func navActions(_ view: DeviceView, device: PairedDevice?, status: HostStatus) -> some View {
        let scope = view.project(id: device?.project)
        if device?.access == .full && status.online {
            Button {
                Haptics.tap()
                Router.shared.push(.newChat(core: core, project: scope?.id))
            } label: {
                IconView(icon: Icons.plus, size: 20)
                    .foregroundStyle(palette.ink)
                    .frame(width: 44, height: 44)
                    .contentShape(Circle())
            }
            .buttonStyle(PressFade())
            .accessibilityLabel("New chat")
        }
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

/// The host in the bar, where the screen it came from would be named: its name and whether it is online, and once
/// the big title has scrolled under the bar, the project after it, which still opens the picker.
private struct HostTitle: View {
    @Environment(\.palette) private var palette
    let name: String
    let online: Bool
    let project: String?
    let shown: Double
    let onPick: (() -> Void)?

    var body: some View {
        HStack(spacing: 6) {
            Text(name)
                .font(Theme.ui(17, .semibold))
                .tracking(-0.3)
                .foregroundStyle(palette.ink)
                .lineLimit(1)
                .layoutPriority(1)
            OnlineDot(online: online)
            if let project, shown > 0 {
                Button {
                    onPick?()
                } label: {
                    HStack(spacing: 5) {
                        Text("/").font(Theme.ui(17)).foregroundStyle(palette.tertiary)
                        Text(project).font(Theme.ui(17, .semibold)).tracking(-0.3).foregroundStyle(palette.ink)
                            .lineLimit(1)
                        if onPick != nil { Chevron(size: 12) }
                    }
                    .frame(minHeight: 44)
                    .contentShape(Rectangle())
                }
                .buttonStyle(PressFade())
                .disabled(onPick == nil)
                .opacity(shown)
                .accessibilityLabel("Project, \(project)")
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityElement(children: .contain)
    }
}

private struct OnlineDot: View {
    @Environment(\.palette) private var palette
    let online: Bool

    var body: some View {
        Group {
            if online {
                Circle().fill(palette.live).frame(width: 7, height: 7).shadow(color: palette.live, radius: 2.5)
            } else {
                Circle().strokeBorder(palette.rest, lineWidth: 1.5).frame(width: 7, height: 7)
            }
        }
        .accessibilityLabel(online ? "Online" : "Offline")
    }
}

/// A downward chevron, as a picker's trigger shows it.
private struct Chevron: View {
    @Environment(\.palette) private var palette
    let size: CGFloat

    var body: some View {
        IconView(icon: Icons.chevron, size: size).foregroundStyle(palette.tertiary).rotationEffect(.degrees(90))
    }
}

/// The big title: the project in view, which opens the picker, with how the host is doing under it.
private struct ProjectHead: View {
    @Environment(\.palette) private var palette
    let title: String
    let line: String
    let provider: String?
    let onPick: (() -> Void)?

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Button {
                onPick?()
            } label: {
                HStack(spacing: 8) {
                    Text(title)
                        .font(Theme.ui(32, .semibold))
                        .tracking(-1.12)
                        .foregroundStyle(palette.ink)
                        .lineLimit(1)
                        .shadow(color: palette.isLight ? .clear : .black.opacity(0.65), radius: 9, y: 2)
                    if let provider { AgentMark(provider: provider, size: 18) }
                    if onPick != nil { Chevron(size: 16).padding(.top, 4) }
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(PressFade())
            .disabled(onPick == nil)
            .accessibilityLabel("Project, \(title)")
            .accessibilityAddTraits(.isHeader)
            Text(line).font(Theme.ui(13)).foregroundStyle(palette.ink.opacity(0.72)).lineLimit(1)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 18)
        .padding(.top, 4)
        .padding(.bottom, 16)
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

/// Agents, Terminals and Calls as one segmented control, each with its icon in its own colour.
private struct HostTabs: View {
    @Environment(\.palette) private var palette
    let tab: Route.DeviceTab
    let calling: Int
    let onSwitch: (Route.DeviceTab) -> Void

    var body: some View {
        HStack(spacing: 2) {
            segment(.agents, "Agents", Icons.agent, palette.accent)
            segment(.terminals, "Terminals", Icons.terminal, palette.warn)
            segment(.calls, CallWords.calls, Icons.message, palette.warn, count: calling)
        }
        .padding(3)
        .background {
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .fill(.ultraThinMaterial)
                .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(palette.raised.opacity(0.6)))
                .overlay(
                    RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(palette.border, lineWidth: 1))
        }
        .padding(.horizontal, 16)
        .padding(.top, 4)
        .padding(.bottom, 10)
    }

    private func segment(
        _ which: Route.DeviceTab, _ label: String, _ icon: IconDef, _ tint: Color, count: Int = 0
    ) -> some View {
        let on = tab == which
        return Button {
            onSwitch(which)
        } label: {
            HStack(spacing: 6) {
                IconView(icon: icon, size: 15).foregroundStyle(tint)
                Text(label)
                    .font(Theme.ui(14, .semibold))
                    .tracking(-0.15)
                    .foregroundStyle(on ? palette.ink : palette.secondary)
                    .lineLimit(1)
                if count > 0 {
                    Text("\(count)")
                        .font(Theme.ui(11, .semibold))
                        .foregroundStyle(palette.ground)
                        .padding(.horizontal, 5)
                        .frame(minWidth: 16, minHeight: 16)
                        .background(Capsule().fill(palette.ink))
                }
            }
            .frame(maxWidth: .infinity, minHeight: 34)
            .background(
                RoundedRectangle(cornerRadius: 9, style: .continuous).fill(on ? palette.active : .clear)
            )
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
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
                if project.standing == .needsYou { Dot(color: palette.warn, size: 7).frame(width: 16, height: 16) }
                if !chats.isEmpty {
                    HStack(spacing: -4) {
                        ForEach(chats.prefix(3), id: \.agentId) { chat in
                            AgentFaceView(
                                agentId: chat.agentId, provider: chat.provider, turn: turn(chat.agentId)?.state,
                                size: 16
                            )
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

    private func turn(_ agentId: String) -> TurnState? {
        view.inbox.agents.first { $0.agentId == agentId }?.turn
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
