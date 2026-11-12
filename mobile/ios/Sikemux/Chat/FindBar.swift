import SwiftUI

/// Find in a chat, as the Mac's find bar does it: the host finds across the whole chat, held here or not, newest
/// first, counts the places a bounded step at a time, and stepping past the places it sent asks for more.
@MainActor @Observable
final class ChatFind {
    /// Places found the host is asked for at a time.
    private static let hits = 200
    /// Counting stops here, and the bar says 999+.
    private static let mostCounted = 1000
    /// How long typing rests before the host is asked.
    private static let typingRest: Duration = .milliseconds(200)

    var text = "" {
        didSet {
            typing?.cancel()
            typing = Task { [weak self] in
                try? await Task.sleep(for: Self.typingRest)
                guard !Task.isCancelled else { return }
                self?.search()
            }
        }
    }
    private(set) var matchCase = false
    private(set) var wholeWord = false
    /// What the rows draw marked, or nil while nothing is looked for.
    private(set) var finding: Finding?

    @ObservationIgnored private let session: ChatSession
    /// Brings a place into view once its row is held.
    @ObservationIgnored private let show: (FindHit) -> Void
    /// The number of the last ask, so an answer to an older query is let go.
    @ObservationIgnored private var asked = 0
    private var found: [FindHit] = []
    /// Where the host goes on from for older places; nil once it has sent them all.
    @ObservationIgnored private var next: FindAt?
    /// The place among `found` the reader is on.
    private var at: Int?
    /// Stepping older went past `found`, and waits for more.
    @ObservationIgnored private var stepping = false
    /// The ask out only counts, so its places are not wanted.
    @ObservationIgnored private var counting = false
    private var counted = 0
    private var complete = false
    /// The last ask failed, so the bar says so rather than that nothing was found.
    private(set) var failed = false
    @ObservationIgnored private var typing: Task<Void, Never>?

    init(session: ChatSession, show: @escaping (FindHit) -> Void) {
        self.session = session
        self.show = show
    }

    /// `3/41…` while the host goes on counting, or why there is nothing to step to.
    var label: String {
        guard !text.isEmpty else { return "" }
        if failed { return String(localized: "Could not search") }
        if counted == 0 { return complete && found.isEmpty ? String(localized: "No results") : "…" }
        if counted >= Self.mostCounted { return "\((at ?? 0) + 1)/999+" }
        let place = at.map { counted - $0 } ?? counted
        return "\(place)/\(counted)\(complete ? "" : "…")"
    }

    func flipCase() {
        matchCase.toggle()
        search()
    }

    func flipWord() {
        wholeWord.toggle()
        search()
    }

    /// Moves to the next place found, older when `older`, across the whole chat.
    func step(older: Bool) {
        guard !found.isEmpty else {
            if older, next != nil { stepping = true }
            return
        }
        let last = found.count - 1
        let to: Int? =
            if let at {
                older ? (at < last ? at + 1 : next == nil ? 0 : nil) : (at > 0 ? at - 1 : next == nil ? last : nil)
            } else {
                0
            }
        guard let to else {
            if older {
                stepping = true
                ask(from: next, countOnly: false)
            }
            return
        }
        at = to
        goTo(to)
        mark()
    }

    func close() {
        typing?.cancel()
        asked += 1
    }

    private func search() {
        typing?.cancel()
        found = []
        next = nil
        at = nil
        stepping = false
        counted = 0
        complete = false
        failed = false
        asked += 1
        if !text.isEmpty { ask(from: nil, countOnly: false) }
        mark()
    }

    private func ask(from: FindAt?, countOnly: Bool) {
        asked += 1
        let asked = asked
        counting = countOnly
        Task {
            do {
                let answer = try await session.find(
                    text, matchCase: matchCase, wholeWord: wholeWord, from: from, direction: .older, limit: Self.hits,
                    countOnly: from == nil ? false : countOnly)
                took(asked, answer)
            } catch {
                guard asked == self.asked else { return }
                complete = true
                failed = true
                Haptics.failure()
                mark()
            }
        }
    }

    /// Takes what the host found, asks it to go on counting until the count is whole or large, and goes on a step
    /// that waited for more.
    private func took(_ asked: Int, _ answer: FindAnswer) {
        guard asked == self.asked else { return }
        failed = false
        if !counting {
            found += answer.hits
            next = answer.next
        }
        counting = false
        counted = max(counted, answer.counted.matches)
        complete = answer.counted.complete
        // A long chat's first slice may hold no place at all, so places are asked for until one comes.
        if found.isEmpty, next != nil {
            ask(from: next, countOnly: false)
            mark()
            return
        }
        let wasStepping = stepping
        stepping = false
        if at == nil, !found.isEmpty {
            at = 0
            goTo(0)
        } else if wasStepping {
            step(older: true)
        }
        if !complete, counted < Self.mostCounted, let more = answer.next, asked == self.asked {
            ask(from: more, countOnly: true)
        }
        mark()
    }

    private func goTo(_ index: Int) {
        let hit = found[index]
        Task {
            guard await session.reach(hit.row), let at, found.indices.contains(at), found[at] == hit else { return }
            show(hit)
        }
    }

    private func mark() {
        guard !found.isEmpty else {
            finding = nil
            return
        }
        var byPart: [String: [Range<Int>]] = [:]
        for hit in found { byPart[hit.part, default: []].append(hit.range.first..<hit.range.second) }
        for key in byPart.keys { byPart[key]?.sort { $0.lowerBound < $1.lowerBound } }
        let current = at.map { found[$0] }
        finding = Finding(byPart: byPart, current: current.map { ($0.part, $0.range.first..<$0.range.second) })
    }
}

/// The find bar in the composer's place: the words, how many places and which one, case and whole-word switches, and
/// steps older and newer.
struct FindBar: View {
    @Environment(\.palette) private var palette
    @Bindable var find: ChatFind
    let onClose: () -> Void
    @FocusState private var focused: Bool

    var body: some View {
        HStack(spacing: 2) {
            HStack(spacing: 7) {
                IconView(icon: Icons.search, size: 14).foregroundStyle(palette.inkFaint)
                TextField("", text: $find.text, prompt: Text("Find in chat").foregroundStyle(palette.inkFaint))
                    .font(Theme.ui(14.5))
                    .foregroundStyle(palette.ink)
                    .tint(palette.accent)
                    .autocorrectionDisabled()
                    .textInputAutocapitalization(.never)
                    .submitLabel(.search)
                    .focused($focused)
                    .onSubmit {
                        find.step(older: true)
                        focused = true
                    }
                    .accessibilityLabel("Find in chat")
                if !find.label.isEmpty {
                    Text(find.label).font(Theme.mono(10.5)).monospacedDigit().foregroundStyle(
                        find.failed ? palette.danger : palette.inkDim)
                }
            }
            .padding(.horizontal, 8)
            .frame(height: 34)
            toggle("Aa", name: "Match case", on: find.matchCase) { find.flipCase() }
            toggle("W", name: "Whole word", on: find.wholeWord) { find.flipWord() }
            step(Icons.arrowUp, label: "Previous match") { find.step(older: true) }
            step(Icons.arrowDown, label: "Next match") { find.step(older: false) }
            step(Icons.close, label: "Close find", action: onClose)
        }
        .padding(5)
        .background(RoundedRectangle(cornerRadius: 16).fill(palette.composer))
        .overlay(RoundedRectangle(cornerRadius: 16).strokeBorder(palette.border, lineWidth: 1))
        .padding(.horizontal, 10)
        .padding(.vertical, 8)
        .onAppear { focused = true }
    }

    private func toggle(_ label: String, name: String, on: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(label).font(Theme.ui(12.5, .semibold)).foregroundStyle(on ? palette.ink : palette.inkDim)
                .frame(width: 32, height: 32)
                .background(RoundedRectangle(cornerRadius: 9).fill(on ? palette.selected : .clear))
        }
        .buttonStyle(ActiveFill(radius: 9))
        .accessibilityLabel(LocalizedStringKey(name))
        .accessibilityValue(on ? Text("on") : Text("off"))
        .accessibilityAddTraits(.isToggle)
    }

    private func step(_ icon: IconDef, label: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            IconView(icon: icon, size: 15).foregroundStyle(palette.inkDim).frame(width: 32, height: 32)
        }
        .buttonStyle(ActiveFill(radius: 9))
        .accessibilityLabel(LocalizedStringKey(label))
    }
}
