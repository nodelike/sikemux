import SwiftUI
import UIKit

/// One call on a page of its own, as the Mac opens a call in its own tab: the call as the page's content, and the
/// composer it is answered with at the bottom.
struct CallScreen: View {
    @Environment(\.palette) private var palette
    @Environment(\.dismiss) private var dismiss
    let core: String
    let id: String
    @State private var refused: String?
    @State private var shown: PersonCall?

    var body: some View {
        let calls = HostCalls.of(core)
        let open = calls.open(in: nil)
        let call = open.first { $0.id == id } ?? shown
        Screen {
            Nav(call.map(CallWords.pageTitle) ?? CallWords.calls)
            if let call {
                let draft = calls.draft(call)
                ScrollView {
                    CallBody(
                        core: core, call: call, draft: draft, canAnswer: canAnswer,
                        next: Self.next(after: call, in: open), report: { refused = $0 }
                    )
                    .padding(.horizontal, 18)
                    .padding(.top, 8)
                    .padding(.bottom, 24)
                    .tapPutsKeyboardAway()
                }
                .scrollDismissesKeyboard(.interactively)
                .safeAreaInset(edge: .bottom, spacing: 0) {
                    VStack(alignment: .leading, spacing: 0) {
                        if let refused, !draft.sending {
                            Text(refused).font(Theme.ui(12.5)).foregroundStyle(palette.danger).padding(.horizontal, 18)
                        }
                        CallComposer(core: core, call: call, draft: draft, canAnswer: canAnswer) { refused = $0 }
                    }
                }
            } else {
                Text(CallWords.noCalls).font(Theme.ui(14)).foregroundStyle(palette.tertiary)
                    .frame(maxWidth: .infinity).padding(.vertical, 24)
            }
        }
        .onChange(of: open.contains { $0.id == id }, initial: true) { _, waiting in
            if waiting {
                shown = open.first { $0.id == id }
            } else if shown != nil {
                dismiss()
            }
        }
    }

    private var canAnswer: Bool { Hub.shared.device(core)?.access != .watch && Hub.shared.live(core).isOpen }

    /// The call after `call` among those waiting, round to the first; none when it is the only one.
    private static func next(after call: PersonCall, in open: [PersonCall]) -> PersonCall? {
        guard let at = open.firstIndex(where: { $0.id == call.id }), open.count > 1 else { return nil }
        return open[(at + 1) % open.count]
    }
}

/// What a call is answered with at the bottom of its page: the chat's composer box, holding the note that goes with
/// the choice made or, with none made, the person's own answer. A call for a secret takes it in a hidden field.
struct CallComposer: View {
    @Environment(\.palette) private var palette
    let core: String
    let call: PersonCall
    @Bindable var draft: CallDraft
    let canAnswer: Bool
    let report: (String) -> Void
    @State private var focused = false

    private var calls: HostCalls { HostCalls.of(core) }
    private var seen: Bool { call.kind != .production || calls.seen.contains(call.id) }
    private var ready: Bool { CallBody.ready(call, draft, canAnswer: canAnswer, seen: seen) }

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: 10, style: .continuous)
        VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .bottom, spacing: 4) {
                field
                Button(action: send) { SendCircle(busy: draft.sending, dim: !ready) }
                    .buttonStyle(PressFade())
                    .disabled(!ready)
                    .padding(.bottom, 4)
                    .accessibilityLabel(String(localized: "Send"))
            }
            if call.secret == true, !draft.typed.isEmpty {
                Text(CallWords.hidden(draft.typed)).font(Theme.ui(12.5)).foregroundStyle(palette.inkDim)
                    .padding(.horizontal, 10).padding(.bottom, 4)
            }
        }
        .padding(6)
        .background(shape.fill(palette.composer))
        .overlay(shape.strokeBorder(focused ? palette.borderSelected : palette.border, lineWidth: 1))
        .padding(.horizontal, 10)
        .padding(.top, 8)
        .padding(.bottom, 6)
        .disabled(!canAnswer)
    }

    @ViewBuilder
    private var field: some View {
        let hint = CallWords.composerHint(call, chosen: draft.chosen(in: call))
        if call.secret == true {
            SecretField(
                text: $draft.typed, focused: $focused, accent: UIColor(palette.accent), dark: !palette.isLight,
                onSubmit: send
            )
            .font(Theme.ui(14.5))
            .frame(minHeight: ComposerInput.minHeight)
            .padding(.horizontal, 10)
            .overlay(alignment: .leading) {
                if draft.typed.isEmpty {
                    Text(hint).font(Theme.ui(14.5)).foregroundStyle(palette.inkFaint).padding(.horizontal, 10)
                        .allowsHitTesting(false)
                }
            }
        } else {
            ComposerInput(text: $draft.typed, placeholder: hint, onFocus: { focused = $0 })
        }
    }

    private func send() {
        guard ready, let params = draft.answer(call) else { return }
        Haptics.tap()
        Keyboard.dismiss()
        calls.answer(call, params, report: report)
    }
}
