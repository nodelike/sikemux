import SwiftUI
import UIKit

/// The phone's paired hosts, the account's hosts it could join, and how to bring another in.
struct DevicesScreen: View {
    @Environment(\.palette) private var palette
    private let hub = Hub.shared
    private let accountHosts = AccountHosts.shared
    @State private var accountShown = false
    /// The slot stays while the list is as long as when + was pressed, so the host that fills it takes its place.
    @State private var addingAt: Int?
    /// Launch opens the only host once.
    private static var launched = false

    var body: some View {
        if let problem = hub.problem {
            PromptScreen(icon: Icons.warning, title: "Couldn't read your paired hosts", detail: problem) {
                ActionButton("Try again", kind: .primary) { Task { await hub.readDevicesAgain() } }
            }
        } else {
            list
        }
    }

    private var list: some View {
        let devices = hub.devices
        let unpaired = accountHosts.hosts.filter { host in !devices.contains { $0.core == host.key } }
        let count = devices.count + unpaired.count
        let adding = addingAt == count
        let empty = count == 0 && accountHosts.loaded
        let listed = accountHosts.loaded && accountHosts.problem == nil
        return Screen {
            HStack(spacing: 8) {
                Text("Devices")
                    .font(Theme.ui(26, .semibold))
                    .tracking(-0.7)
                    .foregroundStyle(palette.ink)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .accessibilityAddTraits(.isHeader)
                IconButton(icon: Icons.plus, label: "Add a host") {
                    Haptics.tap()
                    addingAt = count
                }
                Button {
                    accountShown = true
                } label: {
                    Avatar(size: 28).frame(width: 44, height: 44)
                }
                .buttonStyle(PressFade())
                .accessibilityLabel("Account")
            }
            .padding(.leading, 16)
            .padding(.trailing, 8)
            .frame(height: 56)
            .padding(.bottom, 6)

            ScrollViewReader { scroller in
                ScrollView(showsIndicators: false) {
                    VStack(spacing: 10) {
                        ForEach(devices) { device in
                            DeviceCard(
                                device: device,
                                onAccount: accountHosts.hosts.first { $0.key == device.core },
                                offAccount: listed && !accountHosts.hosts.contains { $0.key == device.core })
                        }
                        ForEach(unpaired, id: \.key) { AccountHostCard(host: $0) }
                        if count == 0 && !accountHosts.loaded { AccountNote(problem: accountHosts.problem) }
                        if empty || adding {
                            HostSlot(onDismiss: adding && count > 0 ? { addingAt = nil } : nil)
                            BringItIn().id("bring")
                        }
                        if count > 0, let problem = accountHosts.problem { AccountNote(problem: problem) }
                    }
                    .padding(.horizontal, 16)
                    .padding(.bottom, bottomGap + 12)
                }
                .refreshable {
                    for device in devices { hub.retry(device.core) }
                    accountHosts.retry()
                    await hub.reloadDevices()
                }
                .onChange(of: adding) { _, now in
                    if now { withAnimation { scroller.scrollTo("bring", anchor: .bottom) } }
                }
            }
        }
        .accountSheet(shown: $accountShown)
        .onAppear {
            guard !Self.launched else { return }
            Self.launched = true
            if devices.count == 1, Router.shared.path.isEmpty { Router.shared.push(.device(core: devices[0].core)) }
        }
    }
}

private struct DeviceCard: View {
    @Environment(\.palette) private var palette
    let device: PairedDevice
    /// The host as the account lists it, whose name the person chose.
    let onAccount: WireDevice?
    /// The account no longer lists it, as after the host was reinstalled.
    let offAccount: Bool

    var body: some View {
        let live = Hub.shared.live(device.core)
        let status = HostStatus(live, device: device)
        // A host being reached again keeps its card lit until a try fails, so cards don't flicker on every retry.
        let away = !status.online && !(status.connecting && status.problem == nil)
        let view = status.online ? live.view : nil
        let working = view?.inbox.working ?? 0
        let background = (view?.inbox.rollup?.subagents ?? 0) + (view?.inbox.rollup?.tasks ?? 0)
        let busy = (view?.inbox.agents ?? []).filter { $0.turn?.state == .working || $0.background }
        let work = [
            working > 0 ? String(localized: "\(working) working") : nil,
            background > 0 ? String(localized: "\(background) in the background") : nil,
        ]
        .compactMap { $0 }.joined(separator: " · ")
        let state =
            offAccount
            ? String(localized: "Not on your account")
            : view.map(HostStatus.summary) ?? (status.online ? String(localized: "Connecting…") : status.line)
        let meta = device.access == .watch ? String(localized: "Watch only · \(state)") : state

        Button {
            Router.shared.push(.device(core: device.core))
        } label: {
            VStack(spacing: 0) {
                HStack(spacing: 14) {
                    IconView(icon: device.kind.icon, size: 26)
                        .foregroundStyle(away ? palette.tertiary : palette.ink)
                        .frame(width: 34, height: 34)
                        .overlay(alignment: .bottomTrailing) {
                            Circle()
                                .fill(away ? palette.ground : palette.live)
                                .overlay(Circle().strokeBorder(away ? palette.rest : palette.raised, lineWidth: 2.5))
                                .frame(width: 10, height: 10)
                                .offset(x: 1, y: -3)
                        }
                    VStack(alignment: .leading, spacing: 2) {
                        Text(onAccount.map(\.name).flatMap { $0.isEmpty ? nil : $0 } ?? device.displayName)
                            .headingStyle(palette)
                            .foregroundStyle(away ? palette.tertiary : palette.ink)
                            .lineLimit(1)
                        Text(device.channelLabel.map { "\($0) · \(meta)" } ?? meta)
                            .metaStyle(palette)
                            .lineLimit(1)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    IconView(icon: Icons.chevron, size: 14).foregroundStyle(palette.rest)
                }
                .padding(.vertical, 14)
                .padding(.leading, 16)
                .padding(.trailing, 14)
                if !busy.isEmpty {
                    HStack(spacing: 10) {
                        HStack(spacing: -5) {
                            ForEach(busy.prefix(4), id: \.agentId) { agent in
                                AgentFaceView(
                                    agentId: agent.agentId, provider: agent.cli, turn: agent.turn, size: 18
                                )
                                .frame(width: 18, height: 18)
                            }
                        }
                        Text(work)
                            .font(Theme.ui(13))
                            .foregroundStyle(palette.secondary)
                            .frame(maxWidth: .infinity, alignment: .leading)
                        if working > 0 { Working() }
                    }
                    .padding(.horizontal, 16)
                    .padding(.top, 12)
                    .padding(.bottom, 14)
                }
            }
            .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(away ? .clear : palette.raised))
            .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).strokeBorder(palette.border, lineWidth: 1))
            .contentShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
        }
        .buttonStyle(CardPress())
        .environment(\.faces, Faces(view))
        .watching(device.core)
    }
}

/// A card dims a little under a finger.
private struct CardPress: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label.opacity(configuration.isPressed ? 0.85 : 1)
    }
}

/// A host signed in to the same account that this phone has not paired with yet: joining it, or why that stopped.
private struct AccountHostCard: View {
    @Environment(\.palette) private var palette
    let host: WireDevice
    @State private var asking = false

    var body: some View {
        let state = HostJoins.shared.states[host.key]
        let failure: JoinFailure? = if case .stopped(let failure) = state { failure } else { nil }
        let since: Date? = if case .joining(let since) = state { since } else { nil }
        let channel = host.channel.flatMap { BuildChannel(rawValue: $0.rawValue) }?.label
        let line =
            failure?.title
            ?? (asking
                ? String(localized: "Waiting for \(host.name) to allow this phone") : String(localized: "Connecting…"))
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 14) {
                IconView(icon: Icons.laptop, size: 26)
                    .foregroundStyle(failure == nil ? palette.ink : palette.tertiary)
                    .frame(width: 34, height: 34)
                    .overlay(alignment: .bottomTrailing) {
                        Circle()
                            .fill(failure == nil ? palette.live : palette.ground)
                            .overlay(
                                Circle().strokeBorder(failure == nil ? palette.raised : palette.rest, lineWidth: 2.5)
                            )
                            .frame(width: 10, height: 10)
                            .offset(x: 1, y: -3)
                    }
                VStack(alignment: .leading, spacing: 2) {
                    Text(host.name).headingStyle(palette)
                        .foregroundStyle(failure == nil ? palette.ink : palette.tertiary).lineLimit(1)
                    Text(channel.map { "\($0) · \(line)" } ?? line).metaStyle(palette).lineLimit(1)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                if failure != nil {
                    Chip(title: "Try again") { HostJoins.shared.retry(host) }
                } else {
                    Working()
                }
            }
            if let failure {
                Text(failure.detail).metaStyle(palette).lineLimit(2).padding(.top, 8).padding(.leading, 48)
            }
        }
        .padding(.vertical, 14)
        .padding(.leading, 16)
        .padding(.trailing, 14)
        .background(
            RoundedRectangle(cornerRadius: 16, style: .continuous).fill(failure == nil ? palette.raised : .clear)
        )
        .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).strokeBorder(palette.border, lineWidth: 1))
        .task(id: since) {
            asking = false
            guard let since else { return }
            try? await Task.sleep(for: .seconds(max(0, HostJoins.askingAfter + since.timeIntervalSinceNow)))
            if !Task.isCancelled { asking = true }
        }
    }
}

/// Where the next host lands: it turns into that host's row once it signs in to the account.
private struct HostSlot: View {
    @Environment(\.palette) private var palette
    var onDismiss: (() -> Void)?

    var body: some View {
        HStack(spacing: 12) {
            IconView(icon: Icons.laptop, size: 22)
                .foregroundStyle(palette.tertiary)
                .frame(width: 40, height: 40)
                .background(RoundedRectangle(cornerRadius: Radius.control, style: .continuous).fill(palette.active))
            VStack(alignment: .leading, spacing: 3) {
                Text("Your computer goes here").font(Theme.row).foregroundStyle(palette.secondary)
                HStack(spacing: 7) {
                    Working()
                    Text("Watching the account").font(Theme.ui(12.5)).foregroundStyle(palette.tertiary)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            if let onDismiss { IconButton(icon: Icons.close, label: "Stop adding a host", action: onDismiss) }
        }
        .padding(14)
        .overlay(
            RoundedRectangle(cornerRadius: Radius.row, style: .continuous)
                .strokeBorder(palette.borderStrong, style: StrokeStyle(lineWidth: 1.5, dash: [5, 4]))
        )
    }
}

/// Where the list stands while the account's hosts are still being read, or could not be.
private struct AccountNote: View {
    @Environment(\.palette) private var palette
    let problem: String?

    var body: some View {
        HStack(spacing: 10) {
            if problem == nil { Working() }
            Text(problem == nil ? "Looking for hosts on your account" : "Couldn't list the hosts on your account.")
                .metaStyle(palette)
                .multilineTextAlignment(.center)
            if problem != nil { Chip(title: "Try again") { AccountHosts.shared.retry() } }
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 24)
        .padding(.horizontal, 8)
    }
}

/// How to bring a host in: install Sikemux there and sign in to this account; the phone then connects by itself.
private struct BringItIn: View {
    @Environment(\.palette) private var palette
    @State private var copied = false
    private static let download = URL(string: "https://sikemux.com/phone")!

    var body: some View {
        let user = Account.shared.user
        let account = [user?.providerName, user?.email].compactMap { $0 }.joined(separator: " · ")
        VStack(alignment: .leading, spacing: 0) {
            Text("Bring it in").font(Theme.ui(15, .semibold)).foregroundStyle(palette.ink)
            VStack(alignment: .leading, spacing: 9) {
                step(1, Text("Get Sikemux from \(strong("sikemux.com"))"))
                step(2, Text("Sign in with \(strong(account.isEmpty ? String(localized: "this account") : account))"))
                step(3, Text("It shows up here and connects by itself"))
            }
            .padding(.top, 10)
            Wrap {
                ShareLink(item: Self.download) { chip("Send me the link", primary: true) }
                    .buttonStyle(CardPress())
                Button {
                    UIPasteboard.general.url = Self.download
                    copied = true
                    Task {
                        try? await Task.sleep(for: .seconds(2))
                        copied = false
                    }
                } label: {
                    chip(copied ? "Copied" : "Copy link")
                }
                .buttonStyle(CardPress())
                Button {
                    Router.shared.push(.hostSignIn)
                } label: {
                    chip("Have a code?")
                }
                .buttonStyle(CardPress())
            }
            .padding(.top, 14)
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: Radius.card, style: .continuous).fill(palette.raised))
        .overlay(
            RoundedRectangle(cornerRadius: Radius.card, style: .continuous).strokeBorder(palette.border, lineWidth: 1)
        )
        .padding(.top, 8)
    }

    private func strong(_ text: String) -> Text {
        Text(text).font(Theme.ui(14, .semibold)).foregroundStyle(palette.ink)
    }

    private func step(_ number: Int, _ text: Text) -> some View {
        HStack(alignment: .top, spacing: 10) {
            Text("\(number)")
                .font(Theme.ui(11.5, .semibold))
                .foregroundStyle(palette.ink)
                .frame(minWidth: 20, minHeight: 20)
                .background(Circle().fill(palette.active))
            text.font(Theme.ui(14)).lineSpacing(3).foregroundStyle(palette.secondary)
        }
    }

    private func chip(_ title: String, primary: Bool = false) -> some View {
        Text(LocalizedStringKey(title))
            .font(Theme.ui(14, .semibold))
            .foregroundStyle(primary ? palette.ground : palette.ink)
            .lineLimit(1)
            .padding(.horizontal, 14)
            .frame(minHeight: 36)
            .background(
                RoundedRectangle(cornerRadius: Radius.control, style: .continuous).fill(primary ? palette.ink : .clear)
            )
            .overlay(
                RoundedRectangle(cornerRadius: Radius.control, style: .continuous).strokeBorder(
                    primary ? palette.ink : palette.borderStrong, lineWidth: 1))
    }
}
