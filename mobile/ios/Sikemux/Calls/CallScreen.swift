import SwiftUI

/// The calls an agent puts to the person, answered one at a time on a screen of their own. It goes back once none is
/// left open.
struct CallScreen: View {
    @Environment(\.palette) private var palette
    @Environment(\.dismiss) private var dismiss
    let core: String
    /// The agent whose calls these are.
    let agent: String
    @State private var refused: (call: String, why: String)?

    var body: some View {
        let calls = HostCalls.of(core)
        let waiting = calls.waiting(for: agent)
        Screen {
            Nav(CallWords.calls)
            if let waiting {
                let draft = calls.draft(waiting)
                ScrollView {
                    VStack(alignment: .leading, spacing: 6) {
                        CallCard(
                            core: core, call: waiting, chat: agent, queue: calls.queue(for: agent), draft: draft,
                            canAnswer: canAnswer, report: { refused = (waiting.id, $0) }
                        )
                        .id(waiting.id)
                        if let refused, refused.call == waiting.id, !draft.sending {
                            Text(refused.why).font(Theme.ui(12.5)).foregroundStyle(palette.danger)
                                .padding(.horizontal, 4)
                        }
                    }
                    .padding(.horizontal, 10)
                    .padding(.top, 8)
                    .padding(.bottom, bottomGap)
                    .tapPutsKeyboardAway()
                }
                .scrollDismissesKeyboard(.interactively)
            }
        }
        .onChange(of: waiting == nil, initial: true) { _, gone in if gone { dismiss() } }
    }

    private var canAnswer: Bool { Hub.shared.device(core)?.access != .watch && Hub.shared.live(core).isOpen }
}
