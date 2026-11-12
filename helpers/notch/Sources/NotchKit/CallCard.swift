import AppKit
import Observation
import SwiftUI

/// What the person has done on a call's card while it waits: the options chosen, the note or
/// their own words, and whether what runs shows.
@Observable
final class CallDraft {
    /// The options chosen; until the person touches the rows, the asker's pick.
    var chosen: [String]?
    var typed = ""
    var actionOpen = false
    var sending = false

    func chosen(in call: PersonCall) -> [String] {
        chosen ?? (call.pick.map { [$0] } ?? [])
    }

    /// Chooses an option, or clears the one chosen; on a call that takes several, ticks it.
    func choose(_ option: String, in call: PersonCall) {
        var now = chosen(in: call)
        if let at = now.firstIndex(of: option) {
            now.remove(at: at)
        } else if call.multi == true {
            now.append(option)
        } else {
            now = [option]
        }
        chosen = now
    }

    /// What answers the call as the card stands: the options chosen with the note, or the
    /// person's own words alone.
    func answer(_ call: PersonCall) -> AnswerCallParams? {
        let words = typed.trimmingCharacters(in: .whitespacesAndNewlines)
        let text = words.isEmpty ? nil : words
        let now = chosen(in: call)
        if now.isEmpty {
            return text.map { AnswerCallParams(id: call.id, words: $0) }
        }
        if call.multi == true {
            return AnswerCallParams(id: call.id, options: now, note: text)
        }
        return AnswerCallParams(id: call.id, option: now[0], note: text)
    }
}

/// Pictures the cards show, read once from this host's disk.
enum CallPictures {
    private static let cache = NSCache<NSString, NSImage>()

    static func image(_ path: String) -> NSImage? {
        if let kept = cache.object(forKey: path as NSString) { return kept }
        guard let image = NSImage(contentsOfFile: path) else { return nil }
        cache.setObject(image, forKey: path as NSString)
        return image
    }
}

/// A call's card, as the Mac draws it: who asks, the question, its context and pictures, the
/// options with the field under them, the point against, and the footer that answers it.
struct CallCard: View {
    let store: NotchStore
    let island: IslandModel
    let call: PersonCall
    @Bindable var draft: CallDraft
    @FocusState private var fieldFocused: Bool

    static let width: CGFloat = 418
    /// Between the card and the island's edge, as the Mac's popover holds it.
    static let margin: CGFloat = 17
    private static let row: CGFloat = 34

    private var chosen: [String] { draft.chosen(in: call) }
    private var typed: Bool { !draft.typed.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
    private var pictured: Bool { call.options.contains { $0.picture != nil } }

    private var context: String? {
        if let brief = call.brief { return "\(brief.tried) \(brief.shows) \(brief.cannotUndo)" }
        return call.context
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            who
            Text(call.question)
                .font(Theme.ui(15, .semibold))
                .foregroundStyle(Theme.ink)
                .lineSpacing(2)
                .padding(.top, 8)
                .padding(.bottom, 3)
            if let context {
                prose(context)
            }
            ForEach(Array((call.evidence ?? []).filter { $0.picture == nil }.enumerated()), id: \.offset) { _, evidence in
                prose(evidence.what).foregroundStyle(Theme.inkDim)
            }
            if pictured {
                strip.padding(.top, 12).padding(.bottom, 14)
            } else if let path = call.evidence?.lazy.compactMap(\.picture).first {
                evidence(path).padding(.top, 12).padding(.bottom, 14)
            } else {
                Color.clear.frame(height: 12)
            }
            options
            if let against = call.against {
                (Text("Against it: ").foregroundStyle(Theme.inkDim) + Text(against).foregroundStyle(Theme.inkSoft))
                    .font(Theme.ui(12.5))
                    .lineSpacing(2)
                    .padding(.top, 12)
            }
            if call.kind == .production, let action = call.action {
                whatRuns(action).padding(.top, 8)
            }
            footer.padding(.top, 14)
        }
        .frame(width: Self.width, alignment: .leading)
        .onAppear { store.saw(call) }
        .onChange(of: typed || fieldFocused, initial: true) { _, held in island.callHeld = held }
    }

    private func prose(_ text: String) -> some View {
        Text(text)
            .font(Theme.ui(12.5))
            .foregroundStyle(Theme.inkSoft)
            .lineSpacing(2)
            .fixedSize(horizontal: false, vertical: true)
    }

    private var who: some View {
        let entry = store.entries.first { $0.agentId == call.asker.agentId }
        return HStack(spacing: 7) {
            AgentLook(face: entry?.agent?.face, provider: call.asker.cli, mark: .needs, size: 20)
            Text("@\(call.asker.name)").font(Theme.ui(12, .medium)).foregroundStyle(Theme.inkSoft)
            if let project = store.projectName(call.project) {
                Text("·").foregroundStyle(Theme.inkFaint)
                Text(project).foregroundStyle(Theme.inkDim)
            }
            Text("·").foregroundStyle(Theme.inkFaint)
            Text(Self.age(call.raisedAt)).foregroundStyle(Theme.inkDim)
        }
        .font(Theme.ui(12))
        .frame(height: 20)
    }

    static func age(_ at: Int, now: Date = .now) -> String {
        let seconds = max(0, Int(now.timeIntervalSince1970) - at / 1000)
        if seconds < 60 { return "now" }
        if seconds < 3600 { return "\(seconds / 60)m" }
        if seconds < 86_400 { return "\(seconds / 3600)h" }
        return "\(seconds / 86_400)d"
    }

    private func picture(_ path: String) -> some View {
        Group {
            if let image = CallPictures.image(path) {
                Image(nsImage: image).resizable().aspectRatio(contentMode: .fill)
            } else {
                Theme.raised
            }
        }
    }

    /// Evidence shows the page from its top, the card's full width.
    private func evidence(_ path: String) -> some View {
        let ratio = CallPictures.image(path).map { $0.size.width / max(1, $0.size.height) } ?? 2
        return picture(path)
            .frame(width: Self.width, height: Self.width / ratio)
            .frame(width: Self.width, height: 148, alignment: .top)
            .clipShape(RoundedRectangle(cornerRadius: 8))
            .overlay(RoundedRectangle(cornerRadius: 8).stroke(Theme.hairline, lineWidth: 1))
    }

    /// A taste call's pictures: the chosen option's leads, large, the others smaller and dimmed
    /// beside it; a click on one chooses it.
    private var strip: some View {
        let lead = chosen.first.flatMap { id in call.options.firstIndex { $0.id == id } }
            ?? call.pick.flatMap { id in call.options.firstIndex { $0.id == id } } ?? 0
        let rest = call.options.indices.filter { $0 != lead }
        return HStack(spacing: 6) {
            tile(call.options[lead]).frame(width: 286)
            VStack(spacing: 6) {
                ForEach(rest, id: \.self) { index in
                    Button {
                        draft.choose(call.options[index].id, in: call)
                    } label: {
                        tile(call.options[index]).opacity(0.72)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(call.options[index].label)
                }
            }
        }
        .frame(width: Self.width, height: 132)
    }

    /// A picture filling its place without widening it: a filled image wants all of its own width.
    private func tile(_ option: CallOption) -> some View {
        Theme.raised
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .overlay {
                if let path = option.picture { picture(path) }
            }
            .clipShape(RoundedRectangle(cornerRadius: 8))
        .overlay(RoundedRectangle(cornerRadius: 8).stroke(Theme.hairline, lineWidth: 1))
    }

    private var options: some View {
        VStack(spacing: 0) {
            ForEach(Array(call.options.enumerated()), id: \.element.id) { index, option in
                if index > 0 { Theme.hairline.frame(height: 1) }
                OptionRow(call: call, option: option, chosen: chosen.contains(option.id), sending: draft.sending) {
                    draft.choose(option.id, in: call)
                }
            }
            if call.secret != true {
                if !call.options.isEmpty { Theme.hairline.frame(height: 1) }
                field
            }
        }
        .clipShape(RoundedRectangle(cornerRadius: 8))
        .overlay(RoundedRectangle(cornerRadius: 8).stroke(fieldLit ? Theme.accentLine : Theme.hairline, lineWidth: 1))
    }

    private var fieldLit: Bool { fieldFocused || typed }

    /// The last row of the options: a note that goes with the option chosen, or with none
    /// chosen, the person's own answer.
    private var field: some View {
        HStack(spacing: 10) {
            IconView(icon: Icons.pencil, size: 11).foregroundStyle(fieldLit ? Theme.inkDim : Theme.inkFaint)
            TextField(chosen.isEmpty ? "Or answer in your own words…" : "Add a note…", text: $draft.typed)
                .textFieldStyle(.plain)
                .font(Theme.ui(13))
                .foregroundStyle(Theme.ink)
                .focused($fieldFocused)
                .onSubmit(goWithThis)
        }
        .padding(.horizontal, 12)
        .frame(height: Self.row)
        .background(Theme.raised)
        .contentShape(Rectangle())
        .onTapGesture {
            island.keyboard?(.take)
            fieldFocused = true
        }
    }

    private func whatRuns(_ action: String) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Button {
                draft.actionOpen.toggle()
            } label: {
                HStack(spacing: 6) {
                    IconView(icon: Icons.chevron, size: 10).rotationEffect(.degrees(draft.actionOpen ? 90 : 0))
                    Text("What runs")
                }
                .font(Theme.ui(12))
                .foregroundStyle(Theme.inkDim)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            if draft.actionOpen {
                Text(action)
                    .font(Theme.mono(12))
                    .foregroundStyle(Theme.inkSoft)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private var footer: some View {
        HStack(spacing: 2) {
            HStack(spacing: 6) {
                IconView(icon: Icons.clock, size: 11)
                Text("Nothing happens until you answer").lineLimit(1)
            }
            .font(Theme.ui(11.5))
            .foregroundStyle(Theme.inkFaint)
            .frame(maxWidth: .infinity, alignment: .leading)
            if call.canHandBack {
                TextButton(title: "Not mine") { store.notMine(call) }
            }
            TextButton(title: "Open chat", icon: Icons.message) { store.focus(call.asker.agentId) }
            if call.secret == true {
                GoButton(title: "Answer in Sikemux", enabled: true) { store.focus(call.asker.agentId) }
            } else {
                GoButton(title: "Go with this", enabled: (!chosen.isEmpty || typed) && !draft.sending, action: goWithThis)
            }
        }
    }

    private func goWithThis() {
        guard !draft.sending, let params = draft.answer(call) else { return }
        store.answer(call, params, draft: draft)
    }
}

private struct OptionRow: View {
    let call: PersonCall
    let option: CallOption
    let chosen: Bool
    let sending: Bool
    let choose: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: choose) {
            HStack(spacing: 10) {
                if call.multi == true {
                    Tick(on: chosen)
                } else {
                    Radio(on: chosen)
                }
                VStack(alignment: .leading, spacing: 1) {
                    Text(option.label).font(Theme.ui(13)).foregroundStyle(Theme.ink)
                    if let detail = option.detail {
                        Text(detail).font(Theme.ui(11.5)).foregroundStyle(Theme.inkDim)
                    }
                }
                Spacer(minLength: 8)
                if call.pick == option.id {
                    HStack(spacing: 5) {
                        IconView(icon: Icons.brand, size: 12).foregroundStyle(Theme.accent)
                        Text("pick")
                    }
                    .font(Theme.ui(11))
                    .foregroundStyle(Theme.inkDim)
                }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 7)
            .frame(minHeight: 34)
            .background(chosen ? Theme.chosen : hovering ? Theme.hover : .clear)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(sending)
        .onHover { hovering = $0 }
    }
}

private struct Radio: View {
    let on: Bool

    var body: some View {
        if on {
            Circle().strokeBorder(Theme.accent, lineWidth: 4).background(Circle().fill(Color.white)).frame(width: 13, height: 13)
        } else {
            Circle().strokeBorder(Theme.inkFaint, lineWidth: 1.5).frame(width: 13, height: 13)
        }
    }
}

private struct Tick: View {
    let on: Bool

    var body: some View {
        if on {
            RoundedRectangle(cornerRadius: 3).fill(Theme.accent).frame(width: 13, height: 13)
                .overlay(IconView(icon: Icons.check, size: 9).foregroundStyle(Theme.onAccent))
        } else {
            RoundedRectangle(cornerRadius: 3).strokeBorder(Theme.inkFaint, lineWidth: 1.5).frame(width: 13, height: 13)
        }
    }
}

/// Words that act, plain until the pointer is on them.
private struct TextButton: View {
    let title: String
    var icon: IconDef?
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 4) {
                if let icon { IconView(icon: icon, size: 11) }
                Text(title)
            }
            .font(Theme.ui(12, .medium))
            .foregroundStyle(hovering ? Theme.ink : Theme.inkSoft)
            .padding(.horizontal, 7)
            .frame(height: 26)
            .background(RoundedRectangle(cornerRadius: 5).fill(hovering ? Theme.hover : .clear))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
    }
}

private struct GoButton: View {
    let title: String
    let enabled: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Text(title)
                .font(Theme.ui(12, .medium))
                .foregroundStyle(Theme.onAccent)
                .padding(.horizontal, 11)
                .frame(height: 26)
                .background(RoundedRectangle(cornerRadius: 5).fill(Theme.accent))
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(!enabled)
        .opacity(enabled ? 1 : 0.5)
    }
}

/// The closed wing's count of calls from every project, the same number the Mac's title bars add up to.
struct CallCount: View {
    let count: Int

    static func width(_ count: Int) -> CGFloat {
        CGFloat(String(count).count) * 7.5 + (count == 1 ? 25 : 31) + 6 + 16
    }

    var body: some View {
        HStack(spacing: 6) {
            Text(count == 1 ? "1 call" : "\(count) calls")
                .font(Theme.ui(12, .semibold))
                .monospacedDigit()
                .foregroundStyle(Theme.warn)
            StateMark(mark: .needs)
        }
    }
}
