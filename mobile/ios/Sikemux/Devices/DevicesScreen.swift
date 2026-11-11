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
                        ForEach(devices) { DeviceCard(device: $0) }
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

    var body: some View {
        let live = Hub.shared.live(device.core)
        let status = HostStatus(live, device: device)
        // A host being reached again keeps its card lit until a try fails, so cards don't flicker on every retry.
        let away = !status.online && !(status.connecting && status.problem == nil)
        let view = status.online ? live.view : nil
        let needs = view?.inbox.items.first.flatMap { $0.tier == .needs ? $0 : nil }
        let working = view?.inbox.working ?? 0
        let background = (view?.inbox.rollup?.subagents ?? 0) + (view?.inbox.rollup?.tasks ?? 0)
        let busy = (view?.inbox.agents ?? []).filter { $0.group == .working || $0.background }
        let work = [working > 0 ? "\(working) working" : nil, background > 0 ? "\(background) in the background" : nil]
            .compactMap { $0 }.joined(separator: " · ")
        let meta = view.map(HostStatus.summary) ?? (status.online ? "Connecting…" : status.line)

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
                        Text(device.displayName)
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
                if let needs {
                    HStack(spacing: 10) {
                        AgentMark(provider: needs.cli ?? "", size: 16)
                        VStack(alignment: .leading, spacing: 1) {
                            Text(
                                needs.title ?? needs.card?.title ?? needs.name.map { "@\($0)" }
                                    ?? Catalog.label(needs.cli ?? "agent", core: device.core)
                            )
                            .font(Theme.ui(13.5, .medium))
                            .foregroundStyle(palette.ink)
                            .lineLimit(1)
                            Text(needs.detail.map { "\(needs.what) · \($0)" } ?? needs.what)
                                .font(Theme.ui(12.5))
                                .foregroundStyle(palette.tertiary)
                                .lineLimit(1)
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                        NeedsYou()
                    }
                    .padding(.vertical, 10)
                    .padding(.horizontal, 12)
                    .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(palette.overlay))
                    .overlay(
                        RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(
                            palette.borderStrong, lineWidth: 1)
                    )
                    .padding(.horizontal, 8)
                    .padding(.bottom, busy.isEmpty ? 8 : 0)
                }
                if !busy.isEmpty {
                    HStack(spacing: 10) {
                        HStack(spacing: -5) {
                            ForEach(busy.prefix(4), id: \.agentId) { agent in
                                AgentMark(provider: agent.cli, size: 18).frame(width: 18, height: 18)
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
        .watching(device.core)
    }
}

/// A card dims a little under a finger.
private struct CardPress: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label.opacity(configuration.isPressed ? 0.85 : 1)
    }
}

/// A host signed in to the same account that this phone has not paired with: it asks that host to let this phone in.
private struct AccountHostCard: View {
    @Environment(\.palette) private var palette
    let host: WireDevice

    var body: some View {
        let channel = host.channel.flatMap { BuildChannel(rawValue: $0.rawValue) }?.label
        Button {
            Haptics.tap()
            Router.shared.push(.join(core: host.key, name: host.name))
        } label: {
            HStack(spacing: 14) {
                IconView(icon: Icons.laptop, size: 26)
                    .foregroundStyle(palette.tertiary)
                    .frame(width: 34, height: 34)
                VStack(alignment: .leading, spacing: 2) {
                    Text(host.name).headingStyle(palette).foregroundStyle(palette.tertiary).lineLimit(1)
                    Text((channel.map { "\($0) · " } ?? "") + "On your account").metaStyle(palette).lineLimit(1)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                Text("Connect")
                    .font(Theme.ui(14, .semibold))
                    .foregroundStyle(palette.ink)
                    .padding(.horizontal, 13)
                    .frame(minHeight: 32)
                    .background(RoundedRectangle(cornerRadius: Radius.control, style: .continuous).fill(palette.raised))
                    .overlay(
                        RoundedRectangle(cornerRadius: Radius.control, style: .continuous).strokeBorder(
                            palette.borderStrong, lineWidth: 1))
            }
            .padding(.vertical, 14)
            .padding(.leading, 16)
            .padding(.trailing, 14)
            .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).strokeBorder(palette.border, lineWidth: 1))
            .contentShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
        }
        .buttonStyle(CardPress())
        .accessibilityLabel("Connect to \(host.name)")
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

/// How to bring a host in: install Sikemux there and sign in to this account; it then asks to let this phone in.
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
                step(2, Text("Sign in with \(strong(account.isEmpty ? "this account" : account))"))
                step(3, Text("Click \(strong("Allow")) when it asks"))
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
        Text(title)
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
