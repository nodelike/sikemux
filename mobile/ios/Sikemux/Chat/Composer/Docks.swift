import SwiftUI
import SikemuxCore

/// One button of a dock: `go` is the one it leads with, `quiet` one that says no.
struct DockAct: Identifiable {
    let id: String
    let label: String
    var go = false
    var quiet = false
    var wide = false
    let onPress: () -> Void
}

/// Sits above the composer until it is answered: a title, the exact command and the answers, with a pager when more
/// than one waits. Wide answers take a row of their own above the rest.
struct Dock: View {
    @Environment(\.palette) private var palette
    struct Pager {
        let index: Int
        let count: Int
        let onPage: (Int) -> Void
        let noun: String
    }

    let title: String
    let detail: String
    var pager: Pager?
    let command: String
    let run: Bool
    var diff: DiffSummary?
    let acts: [DockAct]
    let busy: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .top, spacing: 10) {
                IconView(icon: Icons.shieldBolt, size: 16).foregroundStyle(palette.ink)
                VStack(alignment: .leading, spacing: 1) {
                    Text(title).font(Theme.ui(15, .semibold)).tracking(-0.2).foregroundStyle(palette.ink)
                    Text(detail).font(Theme.ui(12.5)).foregroundStyle(palette.tertiary)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                if let pager, pager.count > 1 { pages(pager) }
            }
            ScrollView(showsIndicators: false) {
                VStack(alignment: .leading, spacing: 4) {
                    (Text(run ? "$ " : "").foregroundColor(palette.toolRun) + Text(command).foregroundColor(palette.ink))
                        .font(Theme.mono(12.5))
                        .lineSpacing(18 - 12.5 * 1.2)
                        .textSelection(.enabled)
                    if let diff {
                        (Text("+\(diff.adds)").foregroundColor(palette.gitAdded) + Text(" ") + Text("−\(diff.dels)").foregroundColor(palette.gitDeleted))
                            .font(Theme.mono(12.5))
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.vertical, 9)
                .padding(.horizontal, 11)
            }
            .scrollBounceBehavior(.basedOnSize)
            .frame(maxHeight: 150)
            .fixedSize(horizontal: false, vertical: true)
            .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(palette.sunken))
            .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(palette.border, lineWidth: 1))
            .padding(.top, 10)
            ForEach(acts.filter(\.wide)) { act in
                HStack(spacing: 6) { button(act) }.padding(.top, 10)
            }
            HStack(spacing: 6) {
                ForEach(acts.filter { !$0.wide }) { act in button(act) }
            }
            .padding(.top, 10)
        }
        .padding(12)
        .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(palette.overlay))
        .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).strokeBorder(palette.borderStrong, lineWidth: 1))
        .padding(.bottom, 8)
        .accessibilityElement(children: .contain)
        .accessibilityLabel("\(title) \(detail)")
    }

    private func button(_ act: DockAct) -> some View {
        Button {
            Haptics.select()
            act.onPress()
        } label: {
            Text(act.label)
                .font(Theme.ui(14, act.go ? .semibold : .medium))
                .foregroundStyle(act.go ? palette.ground : act.quiet ? palette.secondary : palette.ink)
                .lineLimit(1)
                .padding(.horizontal, 6)
                .frame(maxWidth: .infinity, minHeight: 38)
                .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(act.go ? palette.ink : .clear))
                .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(act.go ? palette.ink : palette.borderStrong, lineWidth: 1))
                .contentShape(Rectangle())
        }
        .buttonStyle(PressFade())
        .disabled(busy)
        .opacity(busy ? 0.5 : 1)
    }

    private func pages(_ pager: Pager) -> some View {
        HStack(spacing: 2) {
            pageButton(Icons.chevron, flipped: true, disabled: pager.index == 0, label: "Previous \(pager.noun)") { pager.onPage(pager.index - 1) }
            Text("\(pager.index + 1) of \(pager.count)")
                .font(Theme.ui(12))
                .monospacedDigit()
                .foregroundStyle(palette.tertiary)
                .accessibilityLabel("\(pager.noun.prefix(1).uppercased())\(pager.noun.dropFirst()) \(pager.index + 1) of \(pager.count)")
            pageButton(Icons.chevron, flipped: false, disabled: pager.index == pager.count - 1, label: "Next \(pager.noun)") { pager.onPage(pager.index + 1) }
        }
        .padding(.top, -2)
    }

    private func pageButton(_ icon: IconDef, flipped: Bool, disabled: Bool, label: String, _ action: @escaping () -> Void) -> some View {
        Button(action: action) {
            IconView(icon: icon, size: 12)
                .foregroundStyle(palette.secondary)
                .rotationEffect(.degrees(flipped ? 180 : 0))
                .frame(width: 26, height: 26)
                .contentShape(Rectangle())
        }
        .buttonStyle(PressFade())
        .disabled(disabled)
        .opacity(disabled ? 0.3 : 1)
        .accessibilityLabel(label)
    }
}

/// A chat agent's permission requests, with the agent's own options, and Cancel when none of them says no.
struct PermissionDock: View {
    let requests: [PermissionAsk]
    let provider: String
    let answering: Set<String>
    let onAnswer: (String, String?) -> Void
    @State private var page = 0

    var body: some View {
        let index = min(page, requests.count - 1)
        let request = requests[index]
        let rejects = request.options.filter { $0.kind == .rejectOnce || $0.kind == .rejectAlways }
        let always = request.options.filter { $0.kind == .allowAlways }
        let once = request.options.filter { $0.kind == .allowOnce }
        let cancel = rejects.isEmpty ? [PermissionOption(optionId: "", name: "Cancel", kind: .rejectOnce)] : []
        let primary = once.first ?? always.first
        let title = Self.title(request)
        let agent = Catalog.label(provider)
        Dock(
            title: title,
            detail: "\(agent) needs permission to continue",
            pager: Dock.Pager(index: index, count: requests.count, onPage: { page = $0 }, noun: "request"),
            command: Self.asked(request.toolCall),
            run: Self.kind(request.toolCall) == "run",
            diff: request.diff,
            acts: (cancel + rejects + always + once).map { option in
                DockAct(
                    id: option.optionId.isEmpty ? option.name : option.optionId, label: option.name,
                    go: option == primary, quiet: option.kind == .rejectOnce || option.kind == .rejectAlways,
                    onPress: { onAnswer(request.requestId, option.optionId.isEmpty ? nil : option.optionId) }
                )
            },
            busy: answering.contains(request.requestId)
        )
    }

    static func title(_ request: PermissionAsk) -> String {
        switch kind(request.toolCall) {
        case "run": "Run a command?"
        case "edit", "move", "delete": "Change a file?"
        case "fetch": "Open a web page?"
        default: "Allow this?"
        }
    }

    private static func kind(_ tool: ToolCall) -> String {
        if let verb = tool.row.verb { return verb }
        if let server = tool.row.server { return server }
        switch tool.row.action {
        case .execute: return "run"
        case .switchMode: return "mode"
        case .other, .unknown: return "tool"
        default: return tool.row.action.rawValue
        }
    }

    /// All of what the agent asks to run or touch, since the end of a command can change what it does.
    static func asked(_ tool: ToolCall) -> String {
        if case .object(let input) = tool.rawInput {
            if case .string(let command) = input["command"], !command.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                return command.trimmingCharacters(in: .whitespacesAndNewlines)
            }
            if case .array(let words) = input["command"] {
                let strings = words.compactMap { word -> String? in if case .string(let text) = word { text } else { nil } }
                if strings.count == words.count { return strings.joined(separator: " ") }
            }
        }
        return tool.row.path ?? tool.row.target
    }
}

/// How the person answers a held action, as the host's calls name it.
enum HoldAnswer {
    case refuse, allow, letGo
}

/// An agent's actions that leave the host, held until the person answers: Refuse, Allow once and, where the host
/// offers it, Always allow what it lands in, which leads on a row of its own.
struct HoldDock: View {
    let holds: [ShipHold]
    let answering: Set<String>
    let onAnswer: (ShipHold, HoldAnswer) -> Void
    @State private var page = 0

    private static let titles: [ShipKind: String] = [
        .push: "Push commits?", .merge: "Merge a pull request?", .deploy: "Deploy?", .publish: "Publish?",
        .productionWrite: "Change production data?",
    ]

    var body: some View {
        let index = min(page, holds.count - 1)
        let hold = holds[index]
        var acts: [DockAct] = []
        if let letGo = hold.letGo {
            acts.append(DockAct(
                id: "always", label: "Always allow \(letGo)\(hold.letGoIn.map { " in \($0)" } ?? "")",
                go: true, wide: true, onPress: { onAnswer(hold, .letGo) }
            ))
        }
        acts.append(DockAct(id: "refuse", label: "Refuse", quiet: true, onPress: { onAnswer(hold, .refuse) }))
        acts.append(DockAct(id: "once", label: "Allow once", go: hold.letGo == nil, onPress: { onAnswer(hold, .allow) }))
        return Dock(
            title: Self.titles[hold.kind] ?? "Allow this?",
            detail: hold.why ?? "Sikemux held this until you allow it",
            pager: Dock.Pager(index: index, count: holds.count, onPage: { page = $0 }, noun: "action"),
            command: hold.what,
            run: false,
            acts: acts,
            busy: answering.contains(hold.id)
        )
    }
}

/// The actions held for one agent on a host, and answering them.
@MainActor @Observable
final class Holds {
    private(set) var answering: Set<String> = []

    func answer(_ hold: ShipHold, _ how: HoldAnswer, core: String, report: @escaping (String) -> Void) {
        guard let connection = Hub.shared.live(core).connection else { return }
        answering.insert(hold.id)
        Task {
            do {
                let params = HoldParams(hold: hold.id)
                switch how {
                case .refuse: _ = try await connection.call(PhoneCall.RefuseShip.self, params)
                case .allow: _ = try await connection.call(PhoneCall.AllowShip.self, params)
                case .letGo: _ = try await connection.call(PhoneCall.LetShipGo.self, params)
                }
            } catch {
                report(problem(error))
            }
            answering.remove(hold.id)
        }
    }
}
