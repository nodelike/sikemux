import SikemuxCore
import SwiftUI

/// Forgets a host and goes back to Devices; a failure stays to be shown.
@MainActor @Observable
final class Forgetting {
    private(set) var busy = false

    func leave(_ device: PairedDevice, done: (() -> Void)? = nil) async {
        guard !busy else { return }
        busy = true
        await Hub.shared.forget(device.core)
        HostLook.forget(device)
        Haptics.success()
        done?()
        Router.shared.root()
    }
}

/// A host's options: who else is paired with it, and forgetting it.
struct ForgetSheet: View {
    @Environment(\.palette) private var palette
    let device: PairedDevice
    let onClose: () -> Void
    @State private var forget = Forgetting()

    var body: some View {
        VStack(spacing: 12) {
            VStack(spacing: 12) {
                PromptGlyph(icon: device.kind.icon, small: true)
                Text(device.displayName)
                    .font(Theme.ui(17, .semibold))
                    .tracking(-0.35)
                    .foregroundStyle(palette.ink)
                    .lineLimit(1)
            }
            .padding(.top, 8)
            .padding(.bottom, 4)
            if device.access == "full" {
                ActionButton("Paired devices") {
                    onClose()
                    Router.shared.push(.pairing(core: device.core))
                }
            }
            ActionButton(forget.busy ? "Forgetting…" : "Forget this host", kind: .danger, icon: Icons.trash) {
                Task { await forget.leave(device, done: onClose) }
            }
            .disabled(forget.busy)
            .padding(.top, 8)
        }
        .padding(.horizontal, 4)
    }
}

/// The agents a host's list can be narrowed to, as the provider tiles head the Mac's rail; `all` shows every one.
struct ProviderFilter {
    let offered: [String]
    let chosen: String
    let onChoose: (String) -> Void
}

/// The host's project switcher: the projects open in its app, searched by name or path.
struct ProjectSheet<Tail: View>: View {
    @Environment(\.palette) private var palette
    @Environment(\.hostCore) private var core
    /// The host the projects are open on, named beside the title.
    var device: String?
    let projects: [ProjectInfo]
    /// The chosen project's id, or nil for every project.
    let chosen: String?
    /// Offers every project at once, described by this line.
    var all: String?
    @ViewBuilder var tail: (ProjectInfo) -> Tail
    var providers: ProviderFilter?
    let onChoose: (String?) -> Void
    @State private var query = ""
    @FocusState private var searching: Bool

    var body: some View {
        let needle = query.trimmingCharacters(in: .whitespaces).lowercased()
        let found = projects.filter { needle.isEmpty || "\($0.name) \($0.path)".lowercased().contains(needle) }
        let showAll = all != nil && needle.isEmpty
        VStack(spacing: 0) {
            if let providers { tiles(providers) }
            HStack(alignment: .firstTextBaseline) {
                Text("Projects").font(Theme.ui(17, .semibold)).tracking(-0.35).foregroundStyle(palette.ink)
                Spacer()
                if let device { Text(device).metaStyle(palette) }
            }
            .padding(.horizontal, 4)
            .padding(.top, 2)
            .padding(.bottom, 12)
            HStack(spacing: 10) {
                IconView(icon: Icons.search, size: 16).foregroundStyle(palette.tertiary)
                TextField("", text: $query, prompt: Text("Search projects…").foregroundStyle(palette.tertiary))
                    .font(Theme.ui(15.5))
                    .foregroundStyle(palette.ink)
                    .tint(palette.accent)
                    .autocorrectionDisabled()
                    .textInputAutocapitalization(.never)
                    .submitLabel(.go)
                    .focused($searching)
                    .onSubmit { if let first = found.first { choose(first.id) } }
            }
            .padding(.horizontal, 12)
            .frame(height: 44)
            .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(palette.sunken))
            .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(query.isEmpty ? palette.border : palette.borderSelected, lineWidth: 1))
            VStack(spacing: 0) {
                if showAll {
                    row(on: chosen == nil, divided: false, action: { choose(nil) }) {
                        IconView(icon: Icons.folders, size: 16).foregroundStyle(palette.live)
                        VStack(alignment: .leading, spacing: 1) {
                            Text("All projects").font(Theme.ui(15, .medium)).foregroundStyle(palette.ink)
                            Text(all ?? "").metaStyle(palette)
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                    }
                }
                ForEach(Array(found.enumerated()), id: \.element.id) { index, project in
                    row(on: project.id == chosen, divided: showAll || index > 0, action: { choose(project.id) }) {
                        IconView(icon: Icons.folder, size: 16).foregroundStyle(palette.live)
                        VStack(alignment: .leading, spacing: 1) {
                            Text(project.name).font(Theme.ui(15, .medium)).foregroundStyle(palette.ink)
                            Text(Self.home(project.path))
                                .font(Theme.mono(11.5))
                                .foregroundStyle(palette.tertiary)
                                .lineLimit(1)
                                .truncationMode(.head)
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                        tail(project)
                    }
                }
            }
            .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(palette.raised))
            .clipShape(RoundedRectangle(cornerRadius: 14, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(palette.border, lineWidth: 1))
            .padding(.top, 12)
            if found.isEmpty {
                Text("No open project matches.").metaStyle(palette).frame(maxWidth: .infinity).padding(.top, 24)
            }
        }
        .onDisappear { query = "" }
    }

    private func choose(_ project: String?) {
        query = ""
        searching = false
        onChoose(project)
    }

    private func row<Content: View>(on: Bool, divided: Bool, action: @escaping () -> Void, @ViewBuilder content: () -> Content) -> some View {
        Button(action: action) {
            HStack(spacing: 12) {
                content()
                if on { IconView(icon: Icons.check, size: 16).foregroundStyle(palette.ink) }
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 8)
            .frame(minHeight: 56)
            .background(on ? palette.active : .clear)
            .overlay(alignment: .top) { if divided { Rectangle().fill(palette.border).frame(height: 1) } }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(on ? [.isSelected, .isButton] : .isButton)
    }

    private func tiles(_ filter: ProviderFilter) -> some View {
        HStack(spacing: 6) {
            ForEach(["all"] + filter.offered, id: \.self) { provider in
                let on = provider == filter.chosen
                Button {
                    Haptics.select()
                    filter.onChoose(provider)
                } label: {
                    Group {
                        if provider == "all" {
                            Text("All").font(Theme.ui(13.5, .medium)).foregroundStyle(on ? palette.ink : palette.secondary)
                        } else {
                            AgentMark(provider: provider, size: 20)
                        }
                    }
                    .frame(maxWidth: .infinity, minHeight: 44)
                    .background(RoundedRectangle(cornerRadius: Radius.control, style: .continuous).fill(on ? palette.active : palette.raised))
                    .overlay(RoundedRectangle(cornerRadius: Radius.control, style: .continuous).strokeBorder(on ? palette.borderStrong : palette.border, lineWidth: 1))
                }
                .buttonStyle(.plain)
                .accessibilityLabel(provider == "all" ? "All agents" : agentLabel(provider, core: core))
                .accessibilityAddTraits(on ? [.isSelected] : [])
            }
        }
        .padding(.top, 4)
        .padding(.bottom, 14)
    }

    private static func home(_ path: String) -> String {
        path.replacing(/^\/(Users|home)\/[^\/]+/, with: "~")
    }
}

extension ProjectSheet where Tail == EmptyView {
    init(device: String? = nil, projects: [ProjectInfo], chosen: String?, all: String? = nil, providers: ProviderFilter? = nil,
         onChoose: @escaping (String?) -> Void) {
        self.init(device: device, projects: projects, chosen: chosen, all: all, tail: { _ in EmptyView() }, providers: providers,
                  onChoose: onChoose)
    }
}

/// What was just snoozed or settled from this phone, with how to take it back, for a few seconds.
@MainActor @Observable
final class AwayNote {
    struct Said {
        let text: String
        let undo: () -> Void
    }

    static let shared = AwayNote()
    private(set) var said: Said?
    @ObservationIgnored private var clearing: Task<Void, Never>?

    func tell(_ text: String, undo: @escaping () -> Void) {
        clearing?.cancel()
        said = Said(text: text, undo: undo)
        clearing = Task {
            try? await Task.sleep(for: .seconds(6))
            if !Task.isCancelled { said = nil }
        }
    }

    func clear() {
        clearing?.cancel()
        said = nil
    }
}

struct AwayNoteView: View {
    @Environment(\.palette) private var palette
    private let note = AwayNote.shared

    var body: some View {
        if let said = note.said {
            HStack(spacing: 8) {
                Text(said.text).font(Theme.ui(13.5, .medium)).foregroundStyle(palette.ink)
                Button {
                    Haptics.tap()
                    said.undo()
                    note.clear()
                } label: {
                    Text("Undo")
                        .font(Theme.ui(13.5, .semibold))
                        .foregroundStyle(palette.ink)
                        .padding(.horizontal, 10)
                        .frame(height: 28)
                        .contentShape(Rectangle())
                }
                .buttonStyle(RowPressStyle())
            }
            .padding(.leading, 12)
            .padding(.trailing, 4)
            .frame(height: 36)
            .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(palette.raised))
            .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(palette.border, lineWidth: 1))
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.top, 10)
            .accessibilityElement(children: .contain)
        }
    }
}

/// Snoozing or settling one agent: the host's own times, a time typed in words that the host reads, and Settle; Wake
/// now in place of the times once it is snoozed, and Restore in place of Settle once it is settled.
struct AwaySheet: View {
    @Environment(\.palette) private var palette
    let core: String
    let shown: AgentShown
    let onClose: () -> Void
    @State private var presets: [SnoozePreset] = []
    @State private var typed = ""
    @State private var failed: String?

    var body: some View {
        let snoozed = shown.snoozedUntil != nil
        let settled = shown.settledAt != nil
        VStack(alignment: .leading, spacing: 0) {
            Text(snoozed ? "\(shown.title) is snoozed" : "Snooze \(shown.title)")
                .font(Theme.ui(16, .semibold))
                .tracking(-0.25)
                .foregroundStyle(palette.ink)
                .lineLimit(1)
                .padding(.horizontal, 8)
                .padding(.top, 4)
                .padding(.bottom, 10)
            if snoozed {
                MenuItem(icon: Icons.clock, label: "Wake now") { snooze(until: nil) }.padding(.vertical, 8)
            } else {
                ForEach(presets, id: \.label) { preset in
                    Button { snooze(until: preset.until) } label: {
                        HStack {
                            Text(preset.label).font(Theme.ui(15)).foregroundStyle(palette.inkDim)
                            Spacer()
                            Text(Self.clock(Format.date(millis: preset.until)))
                                .font(Theme.ui(13))
                                .monospacedDigit()
                                .foregroundStyle(palette.tertiary)
                        }
                        .padding(.horizontal, 10)
                        .frame(height: 48)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(RowPressStyle())
                }
                TextField("", text: $typed, prompt: Text("Or type a time: fri 2pm, in 90m").foregroundStyle(palette.tertiary))
                    .font(Theme.ui(15))
                    .foregroundStyle(palette.ink)
                    .tint(palette.accent)
                    .autocorrectionDisabled()
                    .textInputAutocapitalization(.never)
                    .submitLabel(.done)
                    .onSubmit {
                        let words = typed.trimmingCharacters(in: .whitespaces)
                        if !words.isEmpty { snooze(until: nil, words: words) }
                    }
                    .onChange(of: typed) { failed = nil }
                    .padding(.horizontal, 14)
                    .frame(height: 44)
                    .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(palette.sunken))
                    .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(palette.border, lineWidth: 1))
                    .padding(.top, 8)
            }
            if let failed {
                Text(failed).font(Theme.ui(13)).foregroundStyle(palette.danger).padding(.horizontal, 8).padding(.top, 8)
            }
            Group {
                if settled {
                    MenuItem(icon: Icons.rotate, label: "Restore") { act { try await Self.settle($0, [shown.agentId], false) } }
                } else {
                    MenuItem(icon: Icons.check, label: "Settle") {
                        let ids = [shown.agentId]
                        act({ try await Self.settle($0, ids, true) }, said: Said(text: "Settled") { connection in
                            Task { await Self.undo { try await Self.settle(connection, ids, false) } }
                        })
                    }
                }
            }
            .padding(.top, 8)
            .padding(.bottom, 4)
        }
        .task(id: shown.agentId) {
            guard !snoozed, let connection = Hub.shared.live(core).connection else { return }
            do {
                presets = try await connection.call(PhoneCall.SnoozePresets.self, NoParams())
            } catch {
                failed = problem(error)
            }
        }
    }

    private func snooze(until: Int?, words: String? = nil) {
        let ids = [shown.agentId]
        var said: Said?
        if until != nil || words != nil {
            let text = until.map { "Snoozed until \(Self.clock(Format.date(millis: $0)))" } ?? "Snoozed"
            said = Said(text: text) { connection in
                Task { await Self.undo { _ = try await connection.call(PhoneCall.SnoozeAgents.self, SnoozeParams(agentIds: ids, until: nil, typed: nil)) } }
            }
        }
        act({ _ = try await $0.call(PhoneCall.SnoozeAgents.self, SnoozeParams(agentIds: ids, until: until, typed: words)) }, said: said)
    }

    /// What the sheet says it did once it closes, and how to take that back.
    private struct Said {
        let text: String
        let undo: (Connection) -> Void
    }

    private func act(_ work: @escaping (Connection) async throws -> Void, said: Said? = nil) {
        guard let connection = Hub.shared.live(core).connection else {
            Haptics.failure()
            failed = "Not connected to the host."
            return
        }
        Haptics.tap()
        Task {
            do {
                try await work(connection)
                if let said { AwayNote.shared.tell(said.text) { said.undo(connection) } }
                onClose()
            } catch {
                Haptics.failure()
                failed = problem(error)
            }
        }
    }

    /// Takes back what the sheet did, from the note it leaves; a failure buzzes, as the note is gone by then.
    private static func undo(_ work: () async throws -> Void) async {
        do {
            try await work()
        } catch {
            Haptics.failure()
            print("sikemux: could not undo: \(error)")
        }
    }

    static func settle(_ connection: Connection, _ ids: [String], _ settled: Bool) async throws {
        _ = try await connection.call(PhoneCall.SettleAgents.self, SettleParams(agentIds: ids, settled: settled))
    }

    /// The time on this phone's clock, with its weekday when it is not today.
    static func clock(_ at: Date) -> String {
        let time = at.formatted(date: .omitted, time: .shortened)
        return Calendar.current.isDateInToday(at) ? time : "\(at.formatted(.dateTime.weekday(.abbreviated))) \(time)"
    }
}
