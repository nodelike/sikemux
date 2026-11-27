import SwiftUI
import UIKit

/// A call as the Mac's call tab draws it, as the page's own content: who asks, the question, why it came up, then
/// the choices as rows with Go under them. What the person writes goes in the page's composer.
struct CallBody: View {
    @Environment(\.palette) private var palette
    let core: String
    let call: PersonCall
    @Bindable var draft: CallDraft
    let canAnswer: Bool
    let next: PersonCall?
    let report: (String) -> Void
    @State private var failure: Failure?

    private var calls: HostCalls { HostCalls.of(core) }
    private var chosen: [String] { draft.chosen(in: call) }
    private var pictured: Bool { call.secret != true && call.options.contains { $0.picture != nil } }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                CallWho(core: core, call: call)
                Spacer(minLength: 8)
                Button {
                    Haptics.tap()
                    AgentChat.open(core: core, agent: CallWords.desk(call)) { failure = $0 }
                } label: {
                    Text(CallWords.openChat).font(Theme.ui(13.5)).foregroundStyle(palette.secondary)
                        .frame(minHeight: 32).contentShape(Rectangle())
                }
                .buttonStyle(PressFade())
            }
            let asked = CallWords.questionLines(call)
            Text(CallMarks.line(asked.first, strong: palette.ink))
                .font(Theme.ui(21, .semibold))
                .tracking(-0.3)
                .foregroundStyle(palette.ink)
                .textSelection(.enabled)
                .padding(.top, 10)
            ForEach(Array(asked.rest.enumerated()), id: \.offset) { _, line in
                Text(CallMarks.line(line, strong: palette.ink)).font(Theme.ui(15)).foregroundStyle(palette.secondary).textSelection(.enabled)
                    .padding(.top, 10)
            }
            if let link = call.link {
                CallLinkTile(link: link) { if call.kind == .look, canAnswer, !draft.sending { seenLook() } }
                    .padding(.top, 14)
            }
            if let page = call.page {
                PageCard(pages: ([page] + (call.morePages ?? [])).map { PageRef(id: $0.id, title: $0.title) })
                    .padding(.top, 6)
            }
            pictures
            why
            if call.kind != .look { tiles }
            if let against = call.against {
                (Text("\(CallWords.againstIt) ").foregroundStyle(palette.secondary)
                    + Text(CallMarks.line(against, strong: palette.secondary)).foregroundStyle(palette.tertiary))
                    .font(Theme.ui(13.5))
                    .textSelection(.enabled)
                    .padding(.top, 12)
            }
            extras
            if let failed = calls.failed[call.id] {
                Text(CallWords.pushFailed(failed)).font(Theme.ui(13)).foregroundStyle(palette.danger).padding(
                    .top, 10)
            }
            goRow.padding(.top, 14)
        }
        .failureAlert($failure)
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
            .frame(height: 160)
            .padding(.top, 14)
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
            .frame(height: 150)
            .padding(.top, 14)
        }
    }

    private var tiles: some View {
        VStack(spacing: 9) {
            ForEach(call.options, id: \.id) { option in tile(option) }
        }
        .padding(.top, 16)
    }

    /// One choice as a full-width row.
    private func tile(_ option: CallOption) -> some View {
        let on = chosen.contains(option.id)
        let shape = RoundedRectangle(cornerRadius: 12, style: .continuous)
        return VStack(alignment: .leading, spacing: 0) {
            Button {
                Haptics.select()
                draft.choose(option.id, in: call)
            } label: {
                HStack(spacing: 12) {
                    CallChoiceMark(on: on)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(option.label).font(Theme.ui(15.5)).foregroundStyle(palette.ink)
                        if let detail = option.detail {
                            Text(detail).font(Theme.ui(13)).foregroundStyle(palette.tertiary)
                        }
                    }
                    Spacer(minLength: 8)
                    if call.pick == option.id {
                        HStack(spacing: 5) {
                            ChiefMark(size: 13, color: palette.accent)
                            Text(CallWords.pick)
                        }
                        .font(Theme.ui(12.5))
                        .foregroundStyle(palette.tertiary)
                    }
                }
                .padding(.horizontal, 14)
                .padding(.vertical, 11)
                .frame(minHeight: 48)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .disabled(!canAnswer || draft.sending)
            .accessibilityAddTraits(on ? [.isSelected, .isButton] : .isButton)
        }
        .background(shape.fill(on ? palette.active : palette.overlay))
        .overlay(shape.strokeBorder(on ? palette.borderStrong : palette.border, lineWidth: 1))
    }

    @ViewBuilder
    private var extras: some View {
        let runs = call.kind == .production ? call.action : nil
        if runs != nil || call.secret == true {
            VStack(alignment: .leading, spacing: 6) {
                if runs != nil {
                    Button {
                        draft.runsOpen.toggle()
                    } label: {
                        HStack(spacing: 6) {
                            IconView(icon: Icons.chevron, size: 11).rotationEffect(.degrees(draft.runsOpen ? 90 : 0))
                            Text(CallWords.whatRuns)
                        }
                        .font(Theme.ui(13))
                        .foregroundStyle(palette.tertiary)
                        .frame(minHeight: 32)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                }
                if let runs, draft.runsOpen {
                    Text(runs).font(Theme.mono(12.5)).foregroundStyle(palette.secondary).textSelection(.enabled)
                }
                if call.secret == true {
                    Text(CallWords.kept(call.asker.name)).font(Theme.ui(12.5)).foregroundStyle(palette.tertiary)
                }
            }
            .padding(.top, 8)
        }
    }

    @ViewBuilder
    private var why: some View {
        let context = CallWords.context(call)
        let evidence = (call.evidence ?? []).filter { $0.picture == nil }
        if context != nil || !evidence.isEmpty {
            SectionLabel(CallWords.whyItCameUp).padding(.horizontal, -8).padding(.top, 10)
            VStack(alignment: .leading, spacing: 8) {
                if let context {
                    Text(CallMarks.line(context, strong: palette.ink)).font(Theme.ui(14.5)).lineSpacing(3).foregroundStyle(palette.secondary)
                }
                ForEach(Array(evidence.enumerated()), id: \.offset) {
                    Text(CallMarks.line($0.element.what, strong: palette.secondary)).font(Theme.ui(14)).foregroundStyle(palette.tertiary)
                }
            }
            .textSelection(.enabled)
        }
    }

    private var seen: Bool { call.kind != .production || calls.seen.contains(call.id) }

    private var hasGo: Bool { call.kind == .look || call.kind == .do || !call.options.isEmpty }

    private var ready: Bool {
        if call.kind == .look { return canAnswer && !draft.sending }
        if call.kind == .do, chosen.isEmpty { return false }
        return CallBody.ready(call, draft, canAnswer: canAnswer, seen: seen)
    }

    /// Whether a call's composer can send: something chosen or typed, and words when it is not being done, while
    /// nothing is on its way.
    static func ready(_ call: PersonCall, _ draft: CallDraft, canAnswer: Bool, seen: Bool) -> Bool {
        let chosen = draft.chosen(in: call)
        let typed = !draft.typed.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        let answered = typed || (!chosen.isEmpty && !chosen.contains(CallWords.notDoingId))
        return answered && !draft.sending && canAnswer && seen
    }

    private var goWords: String {
        let label = chosen.first.flatMap { id in call.options.first { $0.id == id }?.label }
        switch call.kind {
        case .look: return CallWords.seen
        case .do: return label ?? CallWords.done
        default: return seen ? CallWords.goWithThis : CallWords.connecting
        }
    }

    @ViewBuilder
    private var goRow: some View {
        if hasGo || next != nil {
            HStack(spacing: 8) {
                Spacer(minLength: 0)
                if let next {
                    Button {
                        Haptics.tap()
                        Router.shared.replace(.call(core: core, call: next.id))
                    } label: {
                        Text(CallWords.nextCall).font(Theme.ui(14.5, .medium)).foregroundStyle(palette.secondary)
                            .padding(.horizontal, 14).frame(minHeight: 40).contentShape(Rectangle())
                    }
                    .buttonStyle(PressFade())
                }
                if hasGo {
                    Button(action: goWithThis) {
                        Text(goWords)
                            .font(Theme.ui(15, .semibold))
                            .lineLimit(1)
                            .foregroundStyle(palette.ground)
                            .padding(.horizontal, 16)
                            .frame(minHeight: 40)
                            .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(palette.accent))
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(PressFade())
                    .disabled(!ready)
                    .opacity(ready ? 1 : 0.5)
                }
            }
        }
    }

    private func goWithThis() {
        guard ready else { return }
        if call.kind == .look { return seenLook() }
        guard let params = draft.answer(call) else { return }
        Haptics.select()
        Keyboard.dismiss()
        calls.answer(call, params, report: report)
    }

    private func seenLook() {
        Haptics.select()
        Keyboard.dismiss()
        let note = draft.typed.trimmingCharacters(in: .whitespacesAndNewlines)
        let params = AnswerCallParams(id: call.id, option: CallWords.seenId, note: note.isEmpty ? nil : note)
        calls.answer(call, params, report: report)
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

/// Draws a call's markdown as the Mac's ask sheet does.
enum CallMarks {
    /// One line of a call's words with its bold, italics, code and links drawn, bold in `strong`; a list item gets
    /// its bullet.
    static func line(_ line: String, strong: Color) -> AttributedString {
        var text = line
        var bullet = ""
        if let item = text.range(of: "^[-*+]\\s+", options: .regularExpression) {
            text.removeSubrange(item)
            bullet = "\u{2022} "
        }
        let options = AttributedString.MarkdownParsingOptions(interpretedSyntax: .inlineOnlyPreservingWhitespace)
        var marked = (try? AttributedString(markdown: text, options: options)) ?? AttributedString(text)
        for run in marked.runs where run.inlinePresentationIntent?.contains(.stronglyEmphasized) == true {
            marked[run.range].foregroundColor = strong
        }
        return AttributedString(bullet) + marked
    }
}
