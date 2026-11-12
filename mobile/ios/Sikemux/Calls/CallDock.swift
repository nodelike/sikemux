import SwiftUI
import UIKit

/// A call docked on its asker's chat, above the composer: the Mac's call card at the phone's sizes, a little
/// tighter, with 44-point rows and Go with this across the card. Typing in the composer answers it too, so it has no
/// Open chat. When the agent waits on several calls, the card says which it is and the others wait folded under it.
struct CallDock: View {
    @Environment(\.palette) private var palette
    let core: String
    let call: PersonCall
    /// Every call its asker waits on, oldest first, `call` among them.
    let queue: [PersonCall]
    @Bindable var draft: CallDraft
    let canAnswer: Bool
    let report: (String) -> Void
    @FocusState private var fieldFocused: Bool
    @State private var secretFocused = false

    private static let row: CGFloat = 44

    private var calls: HostCalls { HostCalls.of(core) }
    private var chosen: [String] { draft.chosen(in: call) }
    private var typed: Bool { !draft.typed.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
    private var pictured: Bool { call.secret != true && call.options.contains { $0.picture != nil } }

    var body: some View {
        ScrollView(showsIndicators: false) {
            VStack(alignment: .leading, spacing: 0) {
                CallWho(
                    call: call, place: queue.count > 1 ? queue.firstIndex(of: call).map { ($0 + 1, queue.count) } : nil)
                Text(call.question)
                    .font(Theme.ui(17, .semibold))
                    .tracking(-0.2)
                    .foregroundStyle(palette.ink)
                    .padding(.top, 8)
                if let context = CallWords.context(call) {
                    Text(context).font(Theme.ui(14)).foregroundStyle(palette.tertiary).lineLimit(2).padding(.top, 2)
                }
                ForEach(Array((call.evidence ?? []).filter { $0.picture == nil }.enumerated()), id: \.offset) {
                    Text($0.element.what).font(Theme.ui(14)).foregroundStyle(palette.tertiary).padding(.top, 4)
                }
                pictures
                options
                if let against = call.against {
                    (Text("\(CallWords.againstIt) ").foregroundStyle(palette.tertiary)
                        + Text(against).foregroundStyle(palette.secondary))
                        .font(Theme.ui(13.5))
                        .padding(.top, 10)
                }
                extras
                if let failed = calls.failed[call.id] {
                    Text(CallWords.pushFailed(failed)).font(Theme.ui(13)).foregroundStyle(palette.danger).padding(
                        .top, 10)
                }
                goButton.padding(.top, 12)
                folded
            }
            .padding(12)
        }
        .scrollBounceBehavior(.basedOnSize)
        .frame(maxHeight: 520)
        .fixedSize(horizontal: false, vertical: true)
        .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(palette.overlay))
        .overlay(
            RoundedRectangle(cornerRadius: 16, style: .continuous).strokeBorder(palette.borderStrong, lineWidth: 1)
        )
        .padding(.bottom, 8)
        .task(id: "\(call.id)|\(Hub.shared.live(core).isOpen)") { await calls.see(call) }
    }

    /// The asker's other calls, a line each; a tap brings one into the card.
    @ViewBuilder
    private var folded: some View {
        let others = queue.filter { $0.id != call.id }
        if !others.isEmpty {
            VStack(spacing: 0) {
                ForEach(others, id: \.id) { other in
                    Button {
                        Haptics.select()
                        calls.focus(other)
                    } label: {
                        HStack(spacing: 10) {
                            AskerMark(
                                agentId: other.asker.agentId, provider: other.asker.cli,
                                chief: other.asker.chief == true,
                                turn: .needsYou, size: 16)
                            Text(other.question).font(Theme.ui(14)).foregroundStyle(palette.secondary).lineLimit(1)
                            Spacer(minLength: 8)
                            Text(Format.age(Format.date(millis: other.raisedAt))).font(Theme.ui(12.5))
                                .foregroundStyle(palette.warn)
                        }
                        .frame(minHeight: Self.row)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(PressFade())
                }
            }
            .padding(.top, 8)
        }
    }

    @ViewBuilder
    private var pictures: some View {
        if pictured {
            let lead =
                chosen.first.flatMap { id in call.options.firstIndex { $0.id == id } }
                ?? call.pick.flatMap { id in call.options.firstIndex { $0.id == id } } ?? 0
            HStack(spacing: 6) {
                CallPicture(core: core, path: call.options[lead].picture).frame(maxWidth: .infinity)
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
                            if shown.count > 1 { PictureLabel(text: evidence.what) }
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
            if !call.options.isEmpty { palette.border.frame(height: 1) }
            field
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
                CallChoiceMark(on: on, multi: call.multi == true)
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
    /// on a call for a password, a hidden field.
    private var field: some View {
        HStack(spacing: 12) {
            IconView(icon: Icons.pencil, size: 13).foregroundStyle(
                fieldFocused || secretFocused || typed ? palette.tertiary : palette.inkFaint)
            Group {
                if call.secret == true {
                    SecretField(
                        text: $draft.typed, focused: $secretFocused, accent: UIColor(palette.accent),
                        dark: !palette.isLight
                    )
                    .overlay(alignment: .leading) {
                        if draft.typed.isEmpty {
                            Text(CallWords.secretField).foregroundStyle(palette.tertiary).allowsHitTesting(false)
                        }
                    }
                } else {
                    TextField(chosen.isEmpty ? CallWords.ownWords : CallWords.addNote, text: $draft.typed)
                }
            }
            .font(Theme.ui(15.5))
            .foregroundStyle(palette.ink)
            .focused($fieldFocused)
            .submitLabel(.send)
            .onSubmit(goWithThis)
            .disabled(!canAnswer)
        }
        .padding(.horizontal, 14)
        .frame(minHeight: Self.row)
        .background(palette.sunken.opacity(0.6))
    }

    @ViewBuilder
    private var extras: some View {
        let runs = call.kind == .production ? call.action : nil
        if runs != nil || call.canHandBack || call.secret == true {
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
                    if call.canHandBack && canAnswer {
                        Button(CallWords.notMine) {
                            Haptics.tap()
                            calls.notMine(call, report: report)
                        }
                        .font(Theme.ui(13, .medium))
                        .foregroundStyle(palette.secondary)
                        .frame(minHeight: 32)
                        .disabled(draft.sending)
                    }
                }
                if let runs, draft.runsOpen {
                    Text(runs).font(Theme.mono(12.5)).foregroundStyle(palette.secondary).textSelection(.enabled)
                }
                if call.secret == true {
                    if typed {
                        Text(CallWords.hidden(draft.typed)).font(Theme.ui(12.5)).foregroundStyle(palette.inkDim)
                    }
                    Text(CallWords.kept).font(Theme.ui(12.5)).foregroundStyle(palette.tertiary)
                }
            }
            .padding(.top, 6)
        }
    }

    private var goButton: some View {
        let seen = calls.seen.contains(call.id)
        let ready = (!chosen.isEmpty || typed) && !draft.sending && canAnswer && seen
        return Button(action: goWithThis) {
            Text(seen ? CallWords.goWithThis : CallWords.connecting)
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
        guard let params = draft.answer(call) else { return }
        Haptics.select()
        fieldFocused = false
        calls.answer(call, params, report: report)
    }
}

/// Who asks a call and how long ago, the chief by its own mark.
struct CallWho: View {
    @Environment(\.palette) private var palette
    let call: PersonCall
    /// Which of its asker's calls it is, and of how many, while there are several.
    var place: (Int, Int)?

    var body: some View {
        HStack(spacing: 8) {
            AskerMark(
                agentId: call.asker.agentId, provider: call.asker.cli, chief: call.asker.chief == true,
                turn: .needsYou)
            Text("@\(call.asker.name)").font(Theme.ui(14, .medium)).foregroundStyle(palette.secondary)
            Text("·").foregroundStyle(palette.inkFaint)
            Text(Format.age(Format.date(millis: call.raisedAt))).foregroundStyle(palette.tertiary)
            if let place {
                Spacer(minLength: 8)
                Text(CallWords.place(place.0, of: place.1)).font(Theme.ui(13)).monospacedDigit()
                    .foregroundStyle(palette.tertiary)
            }
        }
        .font(Theme.ui(14))
        .accessibilityElement(children: .combine)
    }
}

/// The radio of an option on a call that takes one, or the box of one on a call that takes several.
struct CallChoiceMark: View {
    @Environment(\.palette) private var palette
    let on: Bool
    let multi: Bool

    var body: some View {
        Group {
            if multi {
                RoundedRectangle(cornerRadius: 4, style: .continuous)
                    .fill(on ? palette.accent : .clear)
                    .overlay(
                        RoundedRectangle(cornerRadius: 4, style: .continuous).strokeBorder(
                            on ? palette.accent : palette.inkFaint, lineWidth: 1.5)
                    )
                    .overlay { if on { IconView(icon: Icons.check, size: 11).foregroundStyle(palette.ground) } }
            } else if on {
                Circle().strokeBorder(palette.accent, lineWidth: 5).background(Circle().fill(Color.white))
            } else {
                Circle().strokeBorder(palette.inkFaint, lineWidth: 1.5)
            }
        }
        .frame(width: 18, height: 18)
    }
}

/// A picture a call names on the host, fetched to this phone. Evidence (`top`) shows a page from its top.
struct CallPicture: View {
    @Environment(\.palette) private var palette
    let core: String
    let path: String?
    var top = false
    @State private var image: UIImage?

    var body: some View {
        palette.sunken
            .overlay(alignment: top ? .topLeading : .center) {
                if let image {
                    Image(uiImage: image).resizable().aspectRatio(contentMode: .fill)
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
            .foregroundStyle(.white)
            .padding(.horizontal, 7)
            .padding(.vertical, 3)
            .background(RoundedRectangle(cornerRadius: 6, style: .continuous).fill(.black.opacity(0.62)))
            .padding(6)
    }
}
