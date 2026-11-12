import SwiftUI

/// The calls a host puts to the person, in the project in view or in every project: those open, each its question
/// with the project and how long it has waited, then those answered lately, with what came of each. A row opens its asker's chat,
/// where an open call docks to be answered. An open call whose asker has no chat on this host, such as an agent in a
/// terminal or a linked host's worker, unfolds its card under its row instead.
struct CallsTab: View {
    @Environment(\.palette) private var palette
    let core: String
    let view: DeviceView
    let scope: ProjectInfo?
    let canAnswer: Bool
    @State private var unfolded: String?
    @State private var refused: (call: String, why: String)?

    var body: some View {
        let calls = HostCalls.of(core)
        let open = calls.open(in: scope)
        let settled = calls.settled(in: scope)
        TimelineView(.periodic(from: .now, by: 60)) { _ in
            VStack(alignment: .leading, spacing: 0) {
                if open.isEmpty {
                    Text(CallWords.noCalls)
                        .font(Theme.ui(14))
                        .foregroundStyle(palette.tertiary)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.vertical, 18)
                        .padding(.horizontal, 4)
                } else {
                    Rows {
                        ForEach(open, id: \.id) { call in
                            row(call) {
                                Text(projectName(call)).foregroundStyle(palette.tertiary).lineLimit(1)
                                Text(Format.age(Format.date(millis: call.raisedAt)))
                                    .monospacedDigit()
                                    .foregroundStyle(palette.warn)
                            }
                            if unfolded == call.id { card(call) }
                        }
                    }
                }
                if !settled.isEmpty {
                    SectionLabel(text: CallWords.answered) { EmptyView() }
                        .padding(.top, 22)
                    Rows {
                        ForEach(settled, id: \.id) { call in
                            row(call) {
                                Text(CallWords.cameOf(call)).foregroundStyle(palette.tertiary).lineLimit(1)
                            }
                        }
                    }
                }
            }
        }
    }

    /// A call as one line: the asker's mark and the question, then at its end what the call's state says.
    private func row<End: View>(_ call: PersonCall, @ViewBuilder end: () -> End) -> some View {
        Row(
            title: call.question,
            onPress: press(call),
            mark: {
                AskerMark(
                    agentId: call.asker.agentId, provider: call.asker.cli, chief: call.asker.chief == true,
                    turn: call.state == .open ? .needsYou : nil)
            },
            end: { HStack(spacing: 8) { end() }.font(Theme.ui(12.5)).fixedSize() }
        )
    }

    /// Opens the call's chat, or unfolds an open call's card when its asker has none; an answered call with no chat
    /// opens nothing.
    private func press(_ call: PersonCall) -> (() -> Void)? {
        guard chatless(call) else {
            return {
                if call.state == .open { HostCalls.of(core).focus(call, in: call.asker.agentId) }
                Router.shared.push(.chat(core: core, agent: call.asker.agentId))
            }
        }
        guard call.state == .open else { return nil }
        return {
            unfolded = unfolded == call.id ? nil : call.id
            refused = nil
        }
    }

    private func chatless(_ call: PersonCall) -> Bool {
        !view.chats.contains { $0.agentId == call.asker.agentId }
    }

    /// The call's card, unfolded under its row at its full height, with why an answer did not go under it while no
    /// other is on its way.
    private func card(_ call: PersonCall) -> some View {
        let draft = HostCalls.of(core).draft(call)
        return VStack(alignment: .leading, spacing: 6) {
            CallDock(
                core: core, call: call, chat: call.asker.agentId, queue: [call], draft: draft,
                canAnswer: canAnswer, report: { refused = (call.id, $0) }, inPlace: true)
            if let refused, refused.call == call.id, !draft.sending {
                Text(refused.why).font(Theme.ui(12.5)).foregroundStyle(palette.danger).padding(.horizontal, 4)
            }
        }
        .padding(.vertical, 8)
    }

    private func projectName(_ call: PersonCall) -> String {
        view.workspace.projects.first { $0.path == call.project }?.name ?? folderName(call.project)
    }
}
