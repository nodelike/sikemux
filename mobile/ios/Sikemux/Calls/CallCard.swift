import SwiftUI
import UIKit

/// A call's card on the Calls screen: the Mac's call card at the phone's sizes, a little tighter, with 44-point rows
/// and Go with this across the card.
struct CallCard: View {
    @Environment(\.palette) private var palette
    let core: String
    let call: PersonCall
    @Bindable var draft: CallDraft
    let canAnswer: Bool
    let report: (String) -> Void
    @FocusState private var fieldFocused: Bool
    @State private var secretFocused = false
    @State private var contextOpen = false

    private static let row: CGFloat = 44

    private var calls: HostCalls { HostCalls.of(core) }
    private var chosen: [String] { draft.chosen(in: call) }
    private var typed: Bool { !draft.typed.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
    private var pictured: Bool { call.secret != true && call.options.contains { $0.picture != nil } }
    private var notDoing: Bool { call.kind == .do && chosen.contains(CallWords.notDoingId) }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            CallWho(core: core, call: call)
            let headline = CallWords.headline(call)
            Text(headline)
                .font(Theme.ui(17, .semibold))
                .tracking(-0.2)
                .foregroundStyle(palette.ink)
                .textSelection(.enabled)
                .padding(.top, 8)
            if headline != call.question {
                Text(call.question).font(Theme.ui(15)).foregroundStyle(palette.secondary).textSelection(.enabled)
                    .padding(.top, 4)
            }
            if let context = CallWords.context(call) {
                // What a step to production, or anything that can't be undone, rests on is read whole; any
                // other call's opens on a tap.
                let whole = call.kind == .production || call.brief?.cannotUndo.isEmpty == false
                Text(context).font(Theme.ui(14)).foregroundStyle(palette.tertiary).textSelection(.enabled)
                    .lineLimit(whole || contextOpen ? nil : 2)
                    .padding(.top, 2)
                    .onTapGesture { if !whole { contextOpen.toggle() } }
                    .accessibilityAddTraits(whole ? [] : .isButton)
                    .accessibilityValue(whole ? "" : contextOpen ? "expanded" : "collapsed")
                    .accessibilityHint(whole ? "" : contextOpen ? CallWords.showLess : CallWords.showAll)
            }
            if let link = call.link {
                CallLinkTile(link: link) { if call.kind == .look, canAnswer, !draft.sending { seenLook() } }
                    .padding(.top, 10)
            }
            if let page = call.page {
                PageCard(page: PageRef(id: page.id, title: page.title))
            }
            ForEach(Array((call.evidence ?? []).filter { $0.picture == nil }.enumerated()), id: \.offset) {
                Text($0.element.what).font(Theme.ui(14)).foregroundStyle(palette.tertiary).textSelection(.enabled)
                    .padding(.top, 4)
            }
            pictures
            if call.kind != .look { options }
            if let against = call.against {
                (Text("\(CallWords.againstIt) ").foregroundStyle(palette.tertiary)
                    + Text(against).foregroundStyle(palette.secondary))
                    .font(Theme.ui(13.5))
                    .textSelection(.enabled)
                    .padding(.top, 10)
            }
            extras
            if let failed = calls.failed[call.id] {
                Text(CallWords.pushFailed(failed)).font(Theme.ui(13)).foregroundStyle(palette.danger).padding(
                    .top, 10)
            }
            goButton.padding(.top, 12)
        }
        .padding(12)
        .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(palette.overlay))
        .overlay(
            RoundedRectangle(cornerRadius: 16, style: .continuous).strokeBorder(palette.borderStrong, lineWidth: 1)
        )
        .padding(.bottom, 8)
        .task(id: "\(call.id)|\(Hub.shared.live(core).isOpen)") { await calls.see(call) }
    }

    @ViewBuilder
    private var pictures: some View {
        if pictured {
            let lead =
                chosen.first.flatMap { id in call.options.firstIndex { $0.id == id } }
                ?? call.pick.flatMap { id in call.options.firstIndex { $0.id == id } } ?? 0
            HStack(spacing: 6) {
                CallPicture(core: core, path: call.options[lead].picture, whole: true).frame(maxWidth: .infinity)
                    .layoutPriority(2)
                VStack(spacing: 6) {
                    ForEach(call.options.indices.filter { $0 != lead }, id: \.self) { index in
                        Button {
                            Haptics.select()
                            draft.choose(call.options[index].id, in: call)
                        } label: {
                            CallPicture(core: core, path: call.options[index].picture).opacity(0.72)
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel(call.options[index].label)
                    }
                }
                .frame(width: 104)
            }
            .frame(height: 120)
            .padding(.top, 10)
        } else if let shown = call.evidence?.filter({ $0.picture != nil }).prefix(2), !shown.isEmpty {
            HStack(spacing: 6) {
                ForEach(Array(shown.enumerated()), id: \.offset) { _, evidence in
                    CallPicture(core: core, path: evidence.picture, top: true)
                        .overlay(alignment: .bottomLeading) {
                            if let label = CallWords.pictureLabel(evidence.what) { PictureLabel(text: label) }
                        }
                        .accessibilityLabel(evidence.what)
                }
            }
            .frame(height: 128)
            .padding(.top, 10)
        }
    }

    private var options: some View {
        VStack(spacing: 0) {
            ForEach(Array(call.options.enumerated()), id: \.element.id) { index, option in
                if index > 0 { palette.border.frame(height: 1) }
                optionRow(option)
            }
            if call.kind != .do || notDoing {
                if !call.options.isEmpty { palette.border.frame(height: 1) }
                field
            }
        }
        .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(
                fieldFocused || secretFocused || typed ? palette.borderSelected : palette.border, lineWidth: 1)
        )
        .padding(.top, 12)
    }

    private func optionRow(_ option: CallOption) -> some View {
        let on = chosen.contains(option.id)
        return Button {
            Haptics.select()
            draft.choose(option.id, in: call)
        } label: {
            HStack(spacing: 12) {
                CallChoiceMark(on: on)
                VStack(alignment: .leading, spacing: 1) {
                    Text(option.label).font(Theme.ui(15.5)).foregroundStyle(palette.ink)
                    if let detail = option.detail {
                        Text(detail).font(Theme.ui(12.5)).foregroundStyle(palette.tertiary)
                    }
                }
                Spacer(minLength: 8)
                if call.pick == option.id {
                    HStack(spacing: 5) {
                        IconView(icon: Icons.brand, size: 13).foregroundStyle(palette.accent)
                        Text(CallWords.pick)
                    }
                    .font(Theme.ui(12.5))
                    .foregroundStyle(palette.tertiary)
                }
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 8)
            .frame(minHeight: Self.row)
            .background(on ? palette.active : .clear)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(!canAnswer || draft.sending)
        .accessibilityAddTraits(on ? [.isSelected, .isButton] : .isButton)
    }

    /// The options' last row: a note that goes with the option chosen, or with none chosen, the person's own answer;
    /// on a call that keeps its words private, a hidden field.
    private var field: some View {
        HStack(spacing: 12) {
            IconView(icon: Icons.pencil, size: 13).foregroundStyle(
                fieldFocused || secretFocused || typed ? palette.tertiary : palette.inkFaint)
            Group {
                if call.secret == true {
                    SecretField(
                        text: $draft.typed, focused: $secretFocused, accent: UIColor(palette.accent),
                        dark: !palette.isLight, onSubmit: goWithThis
                    )
                    .overlay(alignment: .leading) {
                        if draft.typed.isEmpty {
                            Text(chosen.isEmpty ? CallWords.secretField : CallWords.addNote)
                                .foregroundStyle(palette.tertiary).allowsHitTesting(false)
                        }
                    }
                } else {
                    TextField(
                        notDoing ? CallWords.whyNotDoing : chosen.isEmpty ? CallWords.ownWords : CallWords.addNote,
                        text: $draft.typed)
                }
            }
            .font(Theme.ui(15.5))
            .foregroundStyle(palette.ink)
            .focused($fieldFocused)
            // Only the button passes a step to production; Return there sends nothing.
            .submitLabel(call.kind == .production ? .done : .send)
            .onSubmit { if call.kind != .production { goWithThis() } }
            .disabled(!canAnswer)
        }
        .padding(.horizontal, 14)
        .frame(minHeight: Self.row)
        .background(palette.sunken.opacity(0.6))
    }

    @ViewBuilder
    private var extras: some View {
        let runs = call.kind == .production ? call.action : nil
        if runs != nil || call.secret == true {
            VStack(alignment: .leading, spacing: 6) {
                HStack {
                    if runs != nil {
                        Button {
                            draft.runsOpen.toggle()
                        } label: {
                            HStack(spacing: 6) {
                                IconView(icon: Icons.chevron, size: 11).rotationEffect(
                                    .degrees(draft.runsOpen ? 90 : 0))
                                Text(CallWords.whatRuns)
                            }
                            .font(Theme.ui(13))
                            .foregroundStyle(palette.tertiary)
                            .frame(minHeight: 32)
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                    }
                    Spacer(minLength: 0)
                }
                if let runs, draft.runsOpen {
                    Text(runs).font(Theme.mono(12.5)).foregroundStyle(palette.secondary).textSelection(.enabled)
                }
                if call.secret == true {
                    if typed {
                        Text(CallWords.hidden(draft.typed)).font(Theme.ui(12.5)).foregroundStyle(palette.inkDim)
                    }
                    Text(CallWords.kept(call.asker.name)).font(Theme.ui(12.5)).foregroundStyle(palette.tertiary)
                }
            }
            .padding(.top, 6)
        }
    }

    private var seen: Bool { call.kind != .production || calls.seen.contains(call.id) }

    private var ready: Bool {
        let answered: Bool =
            switch call.kind {
            case .look: true
            case .do: !chosen.isEmpty && (!notDoing || typed)
            default: !chosen.isEmpty || typed
            }
        return answered && !draft.sending && canAnswer && seen
    }

    private var goWords: String {
        switch call.kind {
        case .look: CallWords.seen
        case .do: chosen.first.flatMap { id in call.options.first { $0.id == id }?.label } ?? CallWords.done
        default: seen ? CallWords.goWithThis : CallWords.connecting
        }
    }

    private var goButton: some View {
        Button(action: goWithThis) {
            Text(goWords)
                .font(Theme.ui(16, .semibold))
                .foregroundStyle(palette.ground)
                .frame(maxWidth: .infinity, minHeight: 46)
                .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(palette.accent))
                .contentShape(Rectangle())
        }
        .buttonStyle(PressFade())
        .disabled(!ready)
        .opacity(ready ? 1 : 0.5)
    }

    private func goWithThis() {
        guard ready else { return }
        if call.kind == .look { return seenLook() }
        guard let params = draft.answer(call) else { return }
        Haptics.select()
        fieldFocused = false
        calls.answer(call, params, report: report)
    }

    private func seenLook() {
        Haptics.select()
        calls.answer(call, AnswerCallParams(id: call.id, option: CallWords.seenId), report: report)
    }
}

/// Where a do call's task is done, or what a look call shows: the service's mark, the link's words and its address.
struct CallLinkTile: View {
    @Environment(\.palette) private var palette
    @Environment(\.openURL) private var openURL
    let link: CallLink
    let opened: () -> Void

    var body: some View {
        let url = URL(string: link.url)
        Button {
            Haptics.select()
            if let url { openURL(url) }
            opened()
        } label: {
            HStack(spacing: 10) {
                IconView(icon: url.flatMap(ServiceMark.of).flatMap { Icons.named($0) } ?? Icons.globe, size: 14)
                    .foregroundStyle(palette.secondary)
                Text(link.label).font(Theme.ui(14.5, .medium)).foregroundStyle(palette.ink).lineLimit(1)
                    .layoutPriority(1)
                if link.label != link.url {
                    Text(link.url).font(Theme.ui(12.5)).foregroundStyle(palette.tertiary).lineLimit(1)
                }
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 14)
            .frame(minHeight: 44)
            .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(palette.composer))
            .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(palette.border, lineWidth: 1))
            .contentShape(Rectangle())
        }
        .buttonStyle(PressFade())
        .accessibilityAddTraits(.isLink)
    }
}

/// Who asks a call and how long ago, and whom it is about, each as the pill that opens its chat.
struct CallWho: View {
    @Environment(\.palette) private var palette
    @Environment(\.crew) private var crew
    let core: String
    let call: PersonCall

    var body: some View {
        HStack(spacing: 8) {
            AgentPill(agent: named(call.asker), size: 13).fixedSize()
            Text("·").foregroundStyle(palette.inkFaint).accessibilityHidden(true)
            Text(Format.age(Format.date(millis: call.raisedAt))).foregroundStyle(palette.tertiary)
            if let about = call.about {
                Text("·").foregroundStyle(palette.inkFaint).accessibilityHidden(true)
                Text(CallWords.about).foregroundStyle(palette.tertiary)
                AgentPill(agent: named(about), size: 13).fixedSize()
            }
        }
        .font(Theme.ui(14))
        .lineLimit(1)
    }

    /// The agent as this host lists it, or as the call names it once the host no longer does.
    private func named(_ asker: CallAsker) -> Crew.Named {
        crew?.agent(asker.agentId)
            ?? Crew.Named(
                id: asker.agentId, written: "@\(asker.name)", chief: asker.chief == true, provider: asker.cli,
                face: nil, project: nil)
    }
}

/// The radio of an option on a call.
struct CallChoiceMark: View {
    @Environment(\.palette) private var palette
    let on: Bool

    var body: some View {
        Group {
            if on {
                Circle().strokeBorder(palette.accent, lineWidth: 5).background(Circle().fill(Color.white))
            } else {
                Circle().strokeBorder(palette.inkFaint, lineWidth: 1.5)
            }
        }
        .frame(width: 18, height: 18)
    }
}

/// A picture a call names on the host, fetched to this phone. Evidence (`top`) shows a page from its top, and the
/// lead option (`whole`) all of its picture.
struct CallPicture: View {
    @Environment(\.palette) private var palette
    let core: String
    let path: String?
    var top = false
    var whole = false
    @State private var image: UIImage?

    var body: some View {
        palette.sunken
            .overlay(alignment: top ? .topLeading : .center) {
                if let image {
                    Image(uiImage: image).resizable().aspectRatio(contentMode: whole ? .fit : .fill)
                }
            }
            .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(palette.border, lineWidth: 1))
            .task(id: path) {
                image = nil
                guard let path = path.flatMap(HostFiles.imagePath),
                    let file = await HostFiles.shared.read(core: core, path: path)
                else { return }
                image = await Task.detached(priority: .userInitiated) { UIImage(contentsOfFile: file.path) }.value
            }
    }
}

/// A picture's name on its corner, such as Now and New on two evidence pages side by side.
struct PictureLabel: View {
    let text: String

    var body: some View {
        Text(text)
            .font(Theme.ui(12, .medium))
            .lineLimit(1)
            .truncationMode(.tail)
            .foregroundStyle(.white)
            .padding(.horizontal, 7)
            .padding(.vertical, 3)
            .background(RoundedRectangle(cornerRadius: 6, style: .continuous).fill(.black.opacity(0.62)))
            .padding(6)
    }
}
