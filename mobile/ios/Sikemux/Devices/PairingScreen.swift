import SikemuxCore
import SwiftUI

private let accessOptions: [(value: DeviceAccess, label: String, detail: String)] = [
    (.full, "Full control", "Start chats, answer agents, type in terminals"),
    (.watch, "Watch only", "See chats and terminals, answer what agents ask"),
]

private func accessLabel(_ access: DeviceAccess) -> String {
    accessOptions.first { $0.value == access }?.label ?? access.rawValue
}

private func platformName(_ platform: String) -> String {
    ["ios": "iOS", "android": "Android", "macos": "macOS", "linux": "Linux", "web": "Web"][platform] ?? platform
}

private func hostKind(_ platform: String) -> IconDef {
    platform == "linux" ? Icons.desktop : Icons.laptop
}

private func date(_ millis: Int?) -> Date? {
    millis.map { Date(timeIntervalSince1970: Double($0) / 1000) }
}

/// Who may connect to a host: the devices asking now, answered here, and those already paired, changed or removed here.
struct PairingScreen: View {
    let core: String

    var body: some View {
        PairingContent(core: core).watching(core)
    }
}

private struct PairingContent: View {
    @Environment(\.palette) private var palette
    @Environment(\.scenePhase) private var phase
    let core: String
    @State private var pairing: RemotePairing?
    @State private var problem: String?
    @State private var busy: String?
    @State private var editing: String?
    @State private var reads = 0
    @State private var phone: String?
    @State private var arrivals: HostWatch?

    var body: some View {
        let live = Hub.shared.live(core)
        let hostName = Hub.shared.device(core)?.displayName ?? "Host"
        Screen {
            Nav("Paired devices", back: "Back")
            ScrollView(showsIndicators: false) {
                MinuteClock { now in
                    VStack(spacing: 0) {
                        ApprovalRow(core: core)
                        rows(now)
                        if pairing == nil && problem == nil {
                            note("Asking \(hostName)…")
                        }
                        if let problem { note(problem).foregroundStyle(palette.danger) }
                    }
                }
                .padding(.horizontal, 16)
                .padding(.bottom, bottomGap + 24)
            }
            .refreshable { reads += 1 }
            .pausesBackdrop()
        }
        .task(id: "\(live.isOpen) \(reads)") { await read(live) }
        .task {
            do {
                phone = try Identity.shared.deviceIdentity().id()
            } catch {
                print("sikemux: could not read this phone's key: \(error)")
            }
        }
        .onChange(of: phase) { _, now in if now == .active { reads += 1 } }
        .onAppear { arrivals = Cards.onArrive(host: core, kind: .pairing) { reads += 1 } }
        .onDisappear { arrivals?.cancel() }
        .sheet(shown: Binding(get: { editing != nil }, set: { if !$0 { editing = nil } })) {
            if let device = pairing?.devices.first(where: { $0.id == editing }) {
                DeviceSheet(device: device) { access in
                    if access == nil { editing = nil }
                    change(device.id) { try await $0.call(PhoneCall.ChangeDevice.self, DeviceChange(id: device.id, access: access)) }
                }
            }
        }
    }

    @ViewBuilder
    private func rows(_ now: Date) -> some View {
        let pending = pairing?.pending.filter { Double($0.expiresAt) / 1000 > now.timeIntervalSince1970 } ?? []
        let asking = pairing?.pendingLinks.filter { Double($0.expiresAt) / 1000 > now.timeIntervalSince1970 } ?? []
        let links = pairing?.links ?? []
        if !pending.isEmpty {
            SectionLabel("Asking to connect")
            Rows {
                ForEach(pending, id: \.id) { request in
                    Row(title: request.name.isEmpty ? "A phone" : request.name, detail: "\(platformName(request.platform)) · from your account",
                        mark: { IconView(icon: Icons.phone, size: 18).foregroundStyle(palette.ink) }, end: { EmptyView() })
                    chips {
                        Chip(title: "Refuse", busy: busy == request.id) { answer(request.id, nil) }
                        Chip(title: "Watch only", busy: busy == request.id) { answer(request.id, .watch) }
                        Chip(title: "Full control", primary: true, busy: busy == request.id) { answer(request.id, .full) }
                    }
                }
            }
        }
        if let pairing {
            SectionLabel("Paired")
            Rows {
                ForEach(pairing.devices, id: \.id) { paired in
                    let me = paired.id == phone
                    let seen = date(paired.lastSeen).map { "seen \(Format.ago($0, now: now))" } ?? "never connected"
                    Row(
                        title: paired.name.isEmpty ? "Unnamed phone" : paired.name,
                        detail: "\(accessLabel(paired.access)) · \(me ? "this phone" : seen)",
                        onPress: me ? nil : { editing = paired.id },
                        mark: { IconView(icon: Icons.phone, size: 18).foregroundStyle(palette.secondary) },
                        end: { if !me { IconView(icon: Icons.chevron, size: 14).foregroundStyle(palette.rest) } })
                }
            }
        }
        if !asking.isEmpty {
            SectionLabel("Asking to link")
            Rows {
                ForEach(asking, id: \.id) { request in
                    Row(title: request.name.isEmpty ? "A host" : request.name, detail: "\(platformName(request.platform)) · from your account",
                        mark: { IconView(icon: hostKind(request.platform), size: 18).foregroundStyle(palette.ink) }, end: { EmptyView() })
                    chips {
                        Chip(title: "Decline", busy: busy == request.id) { answer(request.id, nil) }
                        Chip(title: "Allow", primary: true, busy: busy == request.id) { answer(request.id, .full) }
                    }
                }
            }
        }
        if !links.isEmpty {
            SectionLabel("Linked hosts")
            Rows {
                ForEach(links, id: \.id) { link in
                    let reach = link.connected ? "connected" : date(link.lastSeen).map { "seen \(Format.ago($0, now: now))" } ?? "never connected"
                    let detail = [platformName(link.platform), reach, link.agents > 0 ? "\(link.agents) \(Format.counted("agent", link.agents))" : nil]
                        .compactMap { $0 }.joined(separator: " · ")
                    Row(title: link.name, detail: detail,
                        mark: { IconView(icon: hostKind(link.platform), size: 18).foregroundStyle(palette.secondary) }, end: { EmptyView() })
                }
            }
        }
    }

    /// Lines under a row start where its text does.
    private func chips<Content: View>(@ViewBuilder _ content: () -> Content) -> some View {
        Wrap { content() }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.leading, 38)
            .padding(.top, 2)
            .padding(.bottom, 8)
    }

    private func note(_ text: String) -> Text {
        Text(text).font(Theme.ui(13)).foregroundColor(palette.tertiary)
    }

    private func read(_ live: HostLive) async {
        guard let connection = live.connection else { return }
        do {
            pairing = try await connection.call(PhoneCall.Pairing.self, NoParams())
            problem = nil
        } catch {
            problem = Sikemux.problem(error)
        }
    }

    private func answer(_ id: String, _ access: DeviceAccess?) {
        change(id) { try await $0.call(PhoneCall.AnswerPairing.self, PairingAnswer(id: id, allow: access != nil, access: access ?? .watch)) }
    }

    private func change(_ id: String, _ ask: @escaping (Connection) async throws -> RemotePairing) {
        guard busy == nil, let connection = Hub.shared.live(core).connection else { return }
        busy = id
        Task {
            defer { busy = nil }
            do {
                pairing = try await ask(connection)
                problem = nil
                Haptics.success()
            } catch {
                Haptics.failure()
                problem = Sikemux.problem(error)
                reads += 1
            }
        }
    }
}

private struct DeviceSheet: View {
    @Environment(\.palette) private var palette
    let device: DeviceInfo
    let onAccess: (DeviceAccess?) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(device.name.isEmpty ? "Unnamed phone" : device.name)
                .font(Theme.ui(17, .semibold))
                .tracking(-0.35)
                .foregroundStyle(palette.ink)
                .lineLimit(1)
            Track(options: accessOptions.map { .init(value: $0.value, label: $0.label) },
                  value: Binding(get: { device.access }, set: { onAccess($0) }))
            Text(accessOptions.first { $0.value == device.access }?.detail ?? "").metaStyle(palette)
            ActionButton("Remove from this host", kind: .danger) { onAccess(nil) }
                .padding(.top, 8)
        }
        .padding(.horizontal, 4)
    }
}

/// The host's lock (ADR 0018): whether each new device and host on the account waits for the person here.
private struct ApprovalRow: View {
    @Environment(\.palette) private var palette
    let core: String
    @State private var on: Bool?
    @State private var problem: String?

    var body: some View {
        HStack(spacing: 12) {
            IconView(icon: Icons.lock, size: 18).foregroundStyle(palette.secondary)
            VStack(alignment: .leading, spacing: 0) {
                Text("Require approval for new devices").font(Theme.ui(15, .medium)).foregroundStyle(palette.ink)
                Text(problem ?? "Ask here before a device or host on your account connects").metaStyle(palette)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            Toggle("Require approval for new devices", isOn: Binding(get: { on ?? false }, set: flip))
                .labelsHidden()
                .tint(palette.live)
                .disabled(on == nil)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 6)
        .frame(minHeight: 52)
        .background(RoundedRectangle(cornerRadius: 9, style: .continuous).fill(palette.raised))
        .overlay(RoundedRectangle(cornerRadius: 9, style: .continuous).strokeBorder(palette.border, lineWidth: 1))
        .padding(.top, 8)
        .task(id: Hub.shared.live(core).isOpen) {
            guard let connection = Hub.shared.live(core).connection else { return }
            do {
                on = try await connection.call(PhoneCall.RequireApproval.self, NoParams()).requireApproval
            } catch {
                problem = Sikemux.problem(error)
            }
        }
    }

    private func flip(_ next: Bool) {
        guard let connection = Hub.shared.live(core).connection else { return }
        on = next
        Task {
            do {
                _ = try await connection.call(PhoneCall.SetRequireApproval.self, ApprovalSetting(requireApproval: next))
                problem = nil
            } catch {
                on = !next
                problem = Sikemux.problem(error)
            }
        }
    }
}
