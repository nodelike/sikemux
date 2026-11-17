import SwiftUI

/// The Now card at the top of a chief's chat, as the Mac floats it: the project's agents in four groups, waiting on
/// the person, ready to look at, moving and not sure, one row each, then the agents closed lately with Reopen. Folded,
/// it is the four counts alone at the top right.
struct NowCard: View {
    @Environment(\.palette) private var palette
    @Environment(\.crew) private var crew
    let core: String
    let now: ProjectNow
    let full: Bool
    let folds: Folds
    let maxHeight: CGFloat

    private static let shown = 4
    private static let closedShown = 3

    private var folded: Bool { !folds.isOpen("now", true) }

    var body: some View {
        let counts = NowCounts(now)
        Group {
            if folded {
                capsule(counts)
            } else {
                card
            }
        }
        .animation(.easeOut(duration: 0.2), value: folded)
    }

    private func fold() {
        Haptics.select()
        folds.set("now", folded)
    }

    // MARK: Folded

    private func capsule(_ counts: NowCounts) -> some View {
        Button(action: fold) {
            HStack(spacing: 4) {
                ForEach(counts.each, id: \.label) { count in
                    HStack(spacing: 6) {
                        Circle().fill(tint(count.tint)).frame(width: 6, height: 6)
                        Text("\(count.n)").font(Theme.ui(13, .semibold)).monospacedDigit()
                    }
                    .foregroundStyle(pillInk(count))
                    .padding(.horizontal, 9)
                    .frame(height: 26)
                    .background(Capsule().fill(count.lit ? palette.accent.opacity(0.16) : .clear))
                }
                chevron(open: false)
            }
            .padding(.leading, 5)
            .padding(.trailing, 4)
            .frame(height: 36)
            .background(surface(Capsule()))
            .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(String(localized: "Now"))
        .frame(maxWidth: .infinity, alignment: .trailing)
        .padding(.horizontal, 14)
    }

    private func pillInk(_ count: NowCounts.Count) -> Color {
        if count.n == 0 { return palette.inkFaint }
        return count.lit ? palette.accent : palette.secondary
    }

    // MARK: Open

    private var card: some View {
        VStack(spacing: 0) {
            head
            ViewThatFits(in: .vertical) {
                rows
                ScrollView { rows }.scrollIndicators(.hidden)
            }
        }
        .frame(maxHeight: maxHeight, alignment: .top)
        .clipShape(RoundedRectangle(cornerRadius: Radius.card, style: .continuous))
        .background(surface(RoundedRectangle(cornerRadius: Radius.card, style: .continuous)))
        .padding(.horizontal, 12)
    }

    private func surface(_ shape: some InsettableShape) -> some View {
        shape.fill(palette.raised)
            .overlay(
                shape.fill(
                    LinearGradient(
                        stops: [
                            .init(color: palette.ink.opacity(0.035), location: 0), .init(color: .clear, location: 0.3),
                        ],
                        startPoint: .top, endPoint: .bottom))
            )
            .overlay(shape.strokeBorder(palette.border))
            .shadow(color: .black.opacity(0.44), radius: 6, y: 8)
            .shadow(color: .black.opacity(0.5), radius: 24, y: 20)
    }

    private var head: some View {
        Button(action: fold) {
            HStack(spacing: 8) {
                IconView(icon: Icons.logo, size: 14).foregroundStyle(palette.accent)
                Text(NowWords.now).font(Theme.ui(14, .semibold)).foregroundStyle(palette.ink)
                Text("· " + NowWords.agents(now.rows?.count ?? 0)).font(Theme.ui(13)).foregroundStyle(palette.tertiary)
                Spacer(minLength: 0)
                chevron(open: true)
            }
            .padding(.leading, 16)
            .padding(.trailing, 8)
            .frame(height: 48)
            .overlay(alignment: .bottom) { Rectangle().fill(palette.border).frame(height: 1) }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    private func chevron(open: Bool) -> some View {
        IconView(icon: Icons.chevron, size: 11)
            .foregroundStyle(palette.tertiary)
            .rotationEffect(.degrees(open ? -90 : 90))
            .frame(width: 28, height: 28)
    }

    private func tint(_ tint: NowCounts.Tint) -> Color {
        switch tint {
        case .you: palette.accent
        case .ready: palette.live
        case .moving: palette.tertiary
        case .unsure: palette.warn
        }
    }

    private var rows: some View {
        VStack(spacing: 10) {
            ForEach([NowGroup.waitingOnYou, .readyToLook, .moving, .notSure], id: \.self) { group in
                listed(group)
            }
            let closed = now.closed ?? []
            if !closed.isEmpty {
                VStack(spacing: 0) {
                    label(NowWords.closedLately, closed.count, tint: palette.tertiary)
                    ForEach(closed.prefix(Self.closedShown), id: \.agent) { closed in
                        NowClosedRow(core: core, closed: closed, full: full)
                    }
                }
            }
        }
        .padding(.horizontal, 8)
        .padding(.top, 6)
        .padding(.bottom, 10)
    }

    @ViewBuilder private func listed(_ group: NowGroup) -> some View {
        let rows = NowCounts.shown(now, group)
        if !rows.isEmpty {
            VStack(spacing: 0) {
                label(NowWords.group(group), rows.count, tint: tint(NowCounts.tint(group)))
                ForEach(rows.prefix(Self.shown), id: \.agent) { row in
                    if let agent = crew?.agent(row.agent) {
                        NowFactRow(agent: agent, row: row, under: NowCounts.workers(now, agent.id))
                    }
                }
                let rest = rows.dropFirst(Self.shown).compactMap { crew?.agent($0.agent) }
                if !rest.isEmpty { NowRestRow(agents: Array(rest)) }
            }
        }
    }

    /// A group's spaced capitals in its colour, how many stand in it, and a hairline to the edge.
    private func label(_ words: String, _ count: Int, tint: Color) -> some View {
        HStack(spacing: 8) {
            Text(words.uppercased()).font(Theme.ui(10.5, .medium)).tracking(2)
                .foregroundStyle(tint).lineLimit(1).fixedSize()
            Text("\(count)").font(Theme.ui(11.5)).monospacedDigit().foregroundStyle(palette.tertiary).fixedSize()
            Rectangle().fill(palette.border).frame(height: 1)
        }
        .padding(.horizontal, 10)
        .frame(height: 26)
    }
}

/// How many of a project's agents stand in each group, counted as the Mac counts them.
struct NowCounts {
    enum Tint { case you, ready, moving, unsure }

    struct Count {
        let n: Int
        let label: String
        let tint: Tint
        /// The person's own count, lit while anything waits on them.
        var lit: Bool { tint == .you && n > 0 }
    }

    let each: [Count]

    static func tint(_ group: NowGroup) -> Tint {
        switch group {
        case .waitingOnYou: .you
        case .readyToLook: .ready
        case .moving: .moving
        case .notSure, .unknown: .unsure
        }
    }

    init(_ now: ProjectNow) {
        each = [
            Count(n: Self.shown(now, .waitingOnYou).count, label: NowWords.forYou, tint: .you),
            Count(n: Self.shown(now, .readyToLook).count, label: NowWords.ready, tint: .ready),
            Count(n: Self.shown(now, .moving).count, label: NowWords.moving, tint: .moving),
            Count(n: Self.shown(now, .notSure).count, label: NowWords.notSure, tint: .unsure),
        ]
    }

    /// The rows of `group` the card counts and lists: a lead's workers stand under their lead, except those waiting on
    /// the person, who each ask for themselves.
    static func shown(_ now: ProjectNow, _ group: NowGroup) -> [NowRow] {
        (now.rows ?? []).filter { row in
            let rowGroup = row.group == .unknown ? .notSure : row.group
            return rowGroup == group && (row.lead == nil || group == .waitingOnYou)
        }
    }

    /// How many of `lead`'s workers stand on the card under it.
    static func workers(_ now: ProjectNow, _ lead: String) -> Int {
        (now.rows ?? []).filter { $0.lead == lead && $0.group != .waitingOnYou }.count
    }
}

/// One agent on the card: its mark, its name, what it is doing and how long it has stood; a tap opens it.
private struct NowFactRow: View {
    @Environment(\.palette) private var palette
    @Environment(\.hostCore) private var core
    let agent: Crew.Named
    let row: NowRow
    let under: Int

    var body: some View {
        let since = Date(timeIntervalSince1970: Double(row.since) / 1000)
        let late = row.group == .waitingOnYou && Date().timeIntervalSince(since) > 10 * 60
        Button {
            Haptics.select()
            Router.shared.push(.chat(core: core, agent: agent.id))
        } label: {
            HStack(spacing: 10) {
                NamedMark(agent: agent, accent: palette.accent, size: 15)
                HStack(alignment: .firstTextBaseline, spacing: 4) {
                    Text(agent.written).font(Theme.ui(13.5, .medium)).foregroundStyle(palette.ink).lineLimit(1)
                    if under > 0 {
                        Text("+\(under)").font(Theme.ui(11.5)).monospacedDigit().foregroundStyle(palette.tertiary)
                            .fixedSize()
                    }
                }
                .frame(width: 96, alignment: .leading)
                Text(NowWords.fact(row.fact)).font(Theme.ui(13.5)).foregroundStyle(palette.secondary).lineLimit(1)
                    .frame(maxWidth: .infinity, alignment: .leading)
                NowClock(since: since, late: late)
            }
            .padding(.horizontal, 10)
            .frame(height: 36)
            .contentShape(Rectangle())
        }
        .buttonStyle(ActiveFill(radius: 8))
    }
}

/// What a group's rows past the first few fold into: their marks and names.
private struct NowRestRow: View {
    @Environment(\.palette) private var palette
    let agents: [Crew.Named]

    var body: some View {
        HStack(spacing: 9) {
            NowMarks(agents: agents)
            Text(agents.map(\.written).joined(separator: ", ")).font(Theme.ui(13.5)).foregroundStyle(
                palette.secondary
            )
            .lineLimit(1)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(.horizontal, 10)
        .frame(height: 36)
    }
}

/// Up to four agents' marks, overlapping.
private struct NowMarks: View {
    @Environment(\.palette) private var palette
    let agents: [Crew.Named]

    var body: some View {
        HStack(spacing: -4) {
            ForEach(agents.prefix(4), id: \.id) { agent in
                NamedMark(agent: agent, accent: palette.accent, size: 12)
                    .frame(width: 20, height: 20)
                    .background(palette.active, in: RoundedRectangle(cornerRadius: 6))
                    .background(palette.raised, in: RoundedRectangle(cornerRadius: 7).inset(by: -2))
            }
        }
    }
}

/// An agent closed lately, why, and Reopen where the phone may act.
private struct NowClosedRow: View {
    @Environment(\.palette) private var palette
    let core: String
    let closed: NowClosed
    let full: Bool
    @State private var sending = false
    @State private var refused: String?

    var body: some View {
        HStack(spacing: 9) {
            Text(refused ?? NowWords.closed(closed)).font(Theme.ui(13.5))
                .foregroundStyle(refused == nil ? palette.secondary : palette.danger)
                .lineLimit(1)
                .frame(maxWidth: .infinity, alignment: .leading)
            if full {
                Button(NowWords.reopen, action: reopen)
                    .buttonStyle(.plain)
                    .font(Theme.ui(13, .medium))
                    .foregroundStyle(palette.ink)
                    .padding(.horizontal, 10)
                    .frame(height: 28)
                    .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(palette.borderStrong))
                    .opacity(sending ? 0.5 : 1)
                    .disabled(sending)
            }
            NowClock(since: Date(timeIntervalSince1970: Double(closed.at) / 1000), late: false)
        }
        .padding(.horizontal, 10)
        .frame(height: 36)
    }

    private func reopen() {
        guard !sending else { return }
        guard let connection = Hub.shared.live(core).connection else {
            refused = CallWords.notConnected
            return
        }
        sending = true
        refused = nil
        Task {
            defer { sending = false }
            do {
                _ = try await connection.call(PhoneCall.ReopenAgent.self, AgentParams(agentId: closed.agent))
                Haptics.success()
            } catch {
                Haptics.failure()
                refused = problem(error)
            }
        }
    }
}

private struct NowClock: View {
    @Environment(\.palette) private var palette
    let since: Date
    let late: Bool

    var body: some View {
        Text(Format.age(since)).font(Theme.ui(12)).monospacedDigit()
            .foregroundStyle(late ? palette.warn : palette.tertiary)
            .fixedSize()
            .frame(minWidth: 30, alignment: .trailing)
    }
}

/// The card's words, as the Mac words them.
enum NowWords {
    static let now = String(localized: "Now")
    static let forYou = String(localized: "For you")
    static let ready = String(localized: "Ready")
    static let moving = String(localized: "Moving")
    static let notSure = String(localized: "Not sure")
    static let closedLately = String(localized: "Closed lately")
    static let reopen = String(localized: "Reopen")

    static func agents(_ count: Int) -> String {
        count == 1 ? String(localized: "1 agent") : String(localized: "\(count) agents")
    }

    static func group(_ group: NowGroup) -> String {
        switch group {
        case .waitingOnYou: String(localized: "Waiting on you")
        case .readyToLook: String(localized: "Ready to look at")
        case .moving: moving
        case .notSure, .unknown: notSure
        }
    }

    /// The one thing a row says of its agent, in a few words.
    static func fact(_ fact: NowFact) -> String {
        switch fact {
        case .asks(let asks):
            asks.count == 1
                ? String(localized: "asks you something") : String(localized: "asked you \(asks.count) things")
        case .failed: String(localized: "its turn failed")
        case .stopped: String(localized: "stopped, nothing set to wake it")
        case .working(let working): working.on ?? String(localized: "working")
        case .waiting(let waiting): waited(waiting)
        case .work(let work): workAt(work.at, release: work.release)
        case .answered: String(localized: "answered")
        case .unknown, .unrecognised: ""
        }
    }

    private static func waited(_ waiting: NowFact.Waiting) -> String {
        if let place = waiting.place { return CallWords.inLine(place) }
        let what = waiting.what ?? ""
        switch waiting.on {
        case .ci: return String(localized: "waiting on CI")
        case .agent where !what.isEmpty: return String(localized: "waiting on @\(what)")
        default: return what.isEmpty ? String(localized: "waiting") : String(localized: "waiting on \(what)")
        }
    }

    private static func workAt(_ at: FixAt, release: String?) -> String {
        switch at {
        case .running: String(localized: "in your app")
        case .released: release.map { String(localized: "in \($0)") } ?? String(localized: "released")
        case .landed: String(localized: "landed")
        case .committed: String(localized: "committed, not landed")
        case .uncommitted: String(localized: "changes not committed")
        case .staged: String(localized: "on staging")
        case .shipped: String(localized: "shipped")
        case .unknown: String(localized: "done")
        }
    }

    /// "Closed @Finral, landed".
    static func closed(_ closed: NowClosed) -> String {
        let why: String =
            switch (closed.why, closed.work) {
            case (.landed, _): String(localized: "landed")
            case (.nothingChanged, _): String(localized: "changed nothing")
            case (.byPerson, _): String(localized: "closed by you")
            case (_, .uncommitted): String(localized: "left changes uncommitted")
            case (_, .committed): String(localized: "committed, not landed")
            default: String(localized: "closed")
            }
        return String(localized: "Closed @\(closed.name), \(why)")
    }
}
