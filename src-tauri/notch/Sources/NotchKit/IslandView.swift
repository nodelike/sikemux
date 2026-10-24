import SwiftUI

/// The island on one screen, top-centred in its panel. Only the black shape
/// is drawn, so the rest of the panel lets clicks through to what is under it.
///
/// It is one shape whose width, height and corners spring together: the band
/// beside the camera swaps what it shows in place, and the body below it
/// arrives scaled from the top. Marks fly from the wings to their rows.
struct IslandView: View {
    let store: NotchStore
    @Bindable var island: IslandModel
    @Namespace private var marks

    var body: some View {
        VStack(spacing: 0) {
            if shows {
                shape.transition(.opacity)
            }
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .compositingGroup()
        .scaleEffect(island.pullScale, anchor: .top)
        .animation(.smooth, value: island.pull)
        .preferredColorScheme(.dark)
        .environment(\.faceMotion, store.view.faceStyle.motion)
        .onChange(of: store.entries) { _, entries in island.entriesChanged(entries) }
        .onChange(of: store.devices) { _, devices in island.devicesChanged(devices) }
    }

    private var geometry: NotchGeometry { island.geometry }

    private var shows: Bool {
        guard !island.hidden else { return false }
        return geometry.hasNotch || !store.rollups.isEmpty || island.mode != .closed
    }

    private var expanded: Bool {
        switch island.mode {
        case .closed, .peekDone: return false
        case .peekAsk, .connect, .open, .drop: return true
        }
    }

    private var radii: (top: CGFloat, bottom: CGFloat) {
        switch island.mode {
        case .closed, .peekDone: return Radii.closed
        case .peekAsk: return Radii.peek
        case .connect, .open, .drop: return Radii.open
        }
    }

    private var width: CGFloat {
        switch island.mode {
        case .closed: return ClosedWings.width(store, geometry)
        case .peekDone: return geometry.notchWidth + 2 * FinishedWings.wing
        case .peekAsk: return 460
        case .connect: return 560
        case .open, .drop: return geometry.hasNotch ? 680 : 560
        }
    }

    private var inset: CGFloat {
        switch island.mode {
        case .closed, .peekDone: return 0
        case .peekAsk: return Radii.peek.top + 8
        case .connect, .open, .drop: return Radii.open.top + 12
        }
    }

    private var lifted: Bool { island.mode != .closed || island.hovering }

    private var shape: some View {
        let outline = NotchShape(top: radii.top, bottom: radii.bottom)
        let shift = island.mode == .closed ? ClosedWings.offset(store, geometry) : 0
        return VStack(alignment: .leading, spacing: 0) {
            band
            if expanded {
                expandedBody.transition(.opening)
            }
        }
        .padding(.horizontal, inset)
        .padding(.bottom, expanded ? 14 : 0)
        .frame(width: width, alignment: .top)
        .background(GeometryReader { proxy in
            Color.clear.onChange(of: proxy.frame(in: .global).offsetBy(dx: shift, dy: 0), initial: true) { _, frame in
                island.shapeFrame = frame
            }
        })
        .background(Color.black)
        .clipShape(outline)
        .contentShape(outline)
        .shadow(color: lifted ? .black.opacity(0.7) : .clear, radius: 6)
        .onHover { island.pointer(entered: $0, opensOnHover: store.settings.openWith == .hover) }
        .onTapGesture {
            if !island.isOpen, !island.asksToConnect { island.set(.open) }
        }
        .onExitCommand { island.set(.closed) }
        .offset(x: shift)
    }

    /// What sits beside the camera: the wings when closed, a header when open.
    @ViewBuilder
    private var band: some View {
        ZStack {
            switch island.mode {
            case .closed:
                ClosedWings(store: store, geometry: geometry, marks: marks)
                    .transition(.inPlace)
            case .peekDone(let id):
                FinishedWings(entry: store.entries.first { $0.id == id }, geometry: geometry, marks: marks)
                    .transition(.inPlace)
            case .peekAsk(let id):
                AskHeader(entry: store.entries.first { $0.id == id }, geometry: geometry, marks: marks)
                    .transition(.inPlace)
            case .connect:
                Color.clear
            case .open:
                OpenHeader(store: store, island: island)
                    .transition(.inPlace)
            case .drop:
                HeaderBand(geometry: geometry) {
                    Text("Drop to start or send").font(Theme.ui(13, .semibold)).foregroundStyle(Theme.ink).padding(.leading, 12)
                } trailing: {
                    EmptyView()
                }
                .transition(.inPlace)
            }
        }
        .frame(maxWidth: .infinity)
        .frame(height: bandHeight)
    }

    /// Without a notch the menu bar is shorter than the header's controls, so an
    /// expanded island takes a notch's height to keep them off the screen edge.
    private var bandHeight: CGFloat {
        expanded && !geometry.hasNotch ? max(geometry.height, 38) : geometry.height
    }

    @ViewBuilder
    private var expandedBody: some View {
        switch island.mode {
        case .peekAsk(let id):
            AskBody(store: store, island: island, entry: store.entries.first { $0.id == id })
        case .connect(let id):
            ConnectCard(store: store, device: store.devices.first { $0.id == id })
        case .drop:
            DropTargets(store: store, island: island)
        default:
            OpenBody(store: store, island: island, marks: marks)
        }
    }
}

extension AnyTransition {
    /// The body arriving in an opening island: from 80%, anchored at the top, as it fades in.
    static var opening: AnyTransition {
        .asymmetric(
            insertion: .scale(scale: 0.8, anchor: .top).combined(with: .opacity).animation(Motion.content),
            removal: .opacity.animation(.smooth(duration: 0.16))
        )
    }

    /// What the band shows changing where it stands: out of and into a 20-point blur.
    static var inPlace: AnyTransition {
        .opacity.combined(with: .modifier(active: BlurModifier(radius: 20), identity: BlurModifier(radius: 0)))
    }
}

private struct BlurModifier: ViewModifier {
    let radius: CGFloat
    func body(content: Content) -> some View { content.blur(radius: radius) }
}

/// Closed: the notch, with the marks of the first items in the inbox on its left
/// and how many are in each state on its right.
struct ClosedWings: View {
    let store: NotchStore
    let geometry: NotchGeometry
    let marks: Namespace.ID

    private var shown: [Entry] { store.entries }

    static let groupGap: CGFloat = 10

    /// Each wing as wide as what it holds, so a quiet right wing stays small
    /// and grows when a state mark arrives.
    static func widths(_ store: NotchStore) -> (left: CGFloat, right: CGFloat) {
        let rollups = store.rollups
        guard !rollups.isEmpty else { return (0, 0) }
        let count = store.entries.count
        var left: CGFloat = 0
        if count > 0 { left = 14 + 20 + CGFloat(min(count, 3) - 1) * 17 + 10 }
        if count > 3 { left += 9 + 6.5 * CGFloat(String(count - 3).count + 1) }
        let groups = rollups.map { ($0.mark.showsCount($0.count) ? CGFloat(String($0.count).count) * 7.5 + 6 : 0) + 16 }
        let held = groups.reduce(0, +) + CGFloat(rollups.count - 1) * ClosedWings.groupGap
        return (left, 10 + held + 14)
    }

    static func width(_ store: NotchStore, _ geometry: NotchGeometry) -> CGFloat {
        let wings = widths(store)
        return geometry.notchWidth + wings.left + wings.right
    }

    /// How far the island moves aside so the camera stays between unequal wings.
    static func offset(_ store: NotchStore, _ geometry: NotchGeometry) -> CGFloat {
        guard geometry.hasNotch else { return 0 }
        let wings = widths(store)
        return (wings.right - wings.left) / 2
    }

    var body: some View {
        let wings = Self.widths(store)
        HStack(spacing: 0) {
            if wings.left > 0 {
                HStack(spacing: -3) {
                    ForEach(shown.prefix(3)) { entry in
                        AgentLook(entry: entry, size: 20)
                            .matchedGeometryEffect(id: entry.id, in: marks)
                    }
                    if shown.count > 3 {
                        Text("+\(shown.count - 3)")
                            .font(Theme.ui(10.5, .semibold))
                            .foregroundStyle(Theme.inkDim)
                            .padding(.leading, 9)
                    }
                }
                .padding(.leading, 14)
                .frame(width: wings.left, alignment: .leading)
            }
            Color.clear.frame(width: geometry.notchWidth)
            if wings.right > 0 {
                HStack(spacing: Self.groupGap) {
                    ForEach(store.rollups, id: \.mark) { group in
                        HStack(spacing: 6) {
                            if group.mark.showsCount(group.count) {
                                Text("\(group.count)").font(Theme.ui(12, .semibold)).monospacedDigit().foregroundStyle(Theme.ink)
                            }
                            switch group.mark {
                            case .mark(let mark): StateMark(mark: mark)
                            case .subagents: RunningGlyph(icon: Icons.agent, size: 10, colour: Theme.live)
                            case .shells: RunningGlyph(icon: Icons.command, size: 11, colour: Theme.warn)
                            }
                        }
                    }
                }
                .padding(.trailing, 14)
                .frame(width: wings.right, alignment: .trailing)
            }
        }
        .frame(height: geometry.height)
    }
}

/// A finished turn, for a moment, on either side of the notch.
struct FinishedWings: View {
    static let wing: CGFloat = 150

    let entry: Entry?
    let geometry: NotchGeometry
    let marks: Namespace.ID

    var body: some View {
        HStack(spacing: 0) {
            HStack(spacing: 8) {
                if let entry {
                    AgentLook(entry: entry, size: 20).matchedGeometryEffect(id: entry.id, in: marks)
                }
                Text(entry?.mark == .failed ? "Failed" : "Finished").font(Theme.ui(12.5, .semibold)).foregroundStyle(Theme.ink)
            }
            .padding(.leading, 14)
            .frame(width: Self.wing, alignment: .leading)
            Color.clear.frame(width: geometry.notchWidth)
            HStack(spacing: 6) {
                Text(entry?.title ?? "")
                    .font(Theme.ui(12.5, .medium))
                    .foregroundStyle(Theme.inkDim)
                    .lineLimit(1)
                StateMark(mark: entry?.mark ?? .done)
            }
            .padding(.trailing, 14)
            .frame(width: Self.wing, alignment: .trailing)
        }
        .frame(height: geometry.height)
    }
}

/// The two sides of the menu-bar band, split around the camera.
struct HeaderBand<Leading: View, Trailing: View>: View {
    let geometry: NotchGeometry
    @ViewBuilder let leading: () -> Leading
    @ViewBuilder let trailing: () -> Trailing

    var body: some View {
        HStack(spacing: 0) {
            HStack(spacing: 8) { leading() }.frame(maxWidth: .infinity, alignment: .leading)
            if geometry.hasNotch { Color.clear.frame(width: geometry.notchWidth + 8) }
            HStack(spacing: 6) { trailing() }.frame(maxWidth: .infinity, alignment: .trailing)
        }
        .frame(height: geometry.height)
    }
}

/// What needs you, in the band: who asks on the left of the camera, that it needs you on the right.
struct AskHeader: View {
    let entry: Entry?
    let geometry: NotchGeometry
    let marks: Namespace.ID

    var body: some View {
        HeaderBand(geometry: geometry) {
            if let entry {
                AgentLook(entry: entry, size: 20).matchedGeometryEffect(id: entry.id, in: marks)
                Text(entry.item.name ?? agentName(entry.provider)).font(Theme.ui(13, .semibold)).foregroundStyle(Theme.ink)
            }
        } trailing: {
            Text("Needs you").font(Theme.ui(12, .semibold)).foregroundStyle(Theme.warn)
            StateMark(mark: .needs)
        }
    }
}

/// What needs you, answered without opening the island.
struct AskBody: View {
    let store: NotchStore
    let island: IslandModel
    let entry: Entry?

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            if let entry {
                VStack(alignment: .leading, spacing: 2) {
                    Text(entry.title).font(Theme.ui(13, .semibold)).foregroundStyle(Theme.ink).lineLimit(1)
                    ProjectLabel(project: entry.project, suffix: entry.what)
                }
                .padding(.top, 8)
                AskDetail(entry: entry).padding(.top, 10)
                AnswerButtons(store: store, island: island, entry: entry, height: 32).padding(.top, 10)
            }
        }
        .padding(.bottom, 2)
    }
}

struct ProjectLabel: View {
    let project: String
    var suffix: String?

    var body: some View {
        HStack(spacing: 4) {
            IconView(icon: Icons.folder, size: 11).foregroundStyle(Theme.inkFaint)
            Text(project).lineLimit(1)
            if let suffix {
                Text("·").foregroundStyle(Theme.inkFaint)
                Text(suffix).lineLimit(1)
            }
        }
        .font(Theme.ui(11.5))
        .foregroundStyle(Theme.inkDim)
    }
}

/// What it waits on: the command it wants to run, or the question or note in its own words.
struct AskDetail: View {
    let entry: Entry
    var darker = false

    var body: some View {
        if let ask = entry.ask {
            CommandText(ask: ask, darker: darker)
        } else if let text = entry.question?.text ?? entry.item.detail {
            Text(text)
                .font(Theme.ui(13))
                .foregroundStyle(Theme.ink)
                .lineLimit(2)
                .truncationMode(.tail)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}

struct CommandText: View {
    let ask: Ask
    var darker = false

    var body: some View {
        HStack(spacing: 8) {
            if ask.isCommand { Text("$").foregroundStyle(Theme.inkFaint) }
            Text(ask.command).foregroundStyle(Theme.ink).lineLimit(1).truncationMode(.tail)
        }
        .font(Theme.mono(12))
        .padding(.horizontal, 12)
        .padding(.vertical, 9)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 10).fill(darker ? Color.black.opacity(0.45) : Theme.raised))
    }
}

/// Deny, Always allow and Allow for a permission; Open in Sikemux for anything the notch cannot answer.
struct AnswerButtons: View {
    let store: NotchStore
    let island: IslandModel
    let entry: Entry
    var height: CGFloat

    var body: some View {
        if let question = entry.question {
            HStack(spacing: 8) {
                if store.settings.answerInNotch, question.choices.count <= 2 {
                    ForEach(Array(question.choices.enumerated()), id: \.offset) { index, choice in
                        CapsuleButton(title: choice.label, height: height) { choose(question, index) }
                    }
                }
                CapsuleButton(title: "Answer in Sikemux", primary: true, height: height) { store.open(entry) }
            }
        } else if let ask = entry.ask, store.settings.answerInNotch, ask.allowOnce != nil || ask.allowAlways != nil {
            HStack(spacing: 8) {
                if let reject = ask.reject {
                    CapsuleButton(title: "Deny", height: height) { answer(ask, reject) }
                }
                if let always = ask.allowAlways {
                    CapsuleButton(title: "Always allow", primary: ask.allowOnce == nil, height: height) { answer(ask, always) }
                }
                if let once = ask.allowOnce {
                    CapsuleButton(title: "Allow", primary: true, height: height) { answer(ask, once) }
                }
            }
        } else {
            CapsuleButton(title: "Open in Sikemux", primary: true, height: height) { store.open(entry) }
        }
    }

    private func answer(_ ask: Ask, _ option: String) {
        store.answer(ask, agentId: entry.item.agentId, optionId: option)
        if case .peekAsk = island.mode { island.set(island.hovering ? .open : .closed) }
    }

    private func choose(_ question: AgentQuestion, _ choice: Int) {
        store.answer(question, choice: choice)
        if case .peekAsk = island.mode { island.set(island.hovering ? .open : .closed) }
    }
}
