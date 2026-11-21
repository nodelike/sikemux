import SwiftUI

/// Every call waiting on the person on one page, as the Mac's Calls view lists them, newest first and each in full,
/// opened at the call the person tapped.
struct CallsScreen: View {
    @Environment(\.palette) private var palette
    let core: String
    /// The call the page opens at.
    let shown: String
    @State private var refused: (call: String, why: String)?

    var body: some View {
        let calls = HostCalls.of(core)
        let open = calls.open(in: nil)
        Screen {
            Nav(CallWords.calls)
            ScrollViewReader { scroller in
                ScrollView {
                    VStack(alignment: .leading, spacing: 6) {
                        if open.isEmpty {
                            Text(CallWords.noCalls).font(Theme.ui(14)).foregroundStyle(palette.tertiary)
                                .padding(.vertical, 18).padding(.horizontal, 4)
                        }
                        ForEach(open, id: \.id) { call in
                            card(call, calls.draft(call))
                        }
                    }
                    .padding(.horizontal, 10)
                    .padding(.top, 8)
                    .padding(.bottom, bottomGap)
                    .tapPutsKeyboardAway()
                }
                .scrollDismissesKeyboard(.interactively)
                .onAppear { scroller.scrollTo(shown, anchor: .top) }
            }
        }
    }

    private func card(_ call: PersonCall, _ draft: CallDraft) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            CallCard(core: core, call: call, draft: draft, canAnswer: canAnswer, report: { refused = (call.id, $0) })
            if let refused, refused.call == call.id, !draft.sending {
                Text(refused.why).font(Theme.ui(12.5)).foregroundStyle(palette.danger).padding(.horizontal, 4)
            }
        }
        .id(call.id)
    }

    private var canAnswer: Bool { Hub.shared.device(core)?.access != .watch && Hub.shared.live(core).isOpen }
}
