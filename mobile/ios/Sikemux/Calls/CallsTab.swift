import SwiftUI

/// The calls a host puts to the person, in the project in view or in every project: those open, each its question
/// with the project and how long it has waited, then those answered lately, with what came of each. An open call's
/// row opens it to be answered, and an answered one's its asker's chat.
struct CallsTab: View {
    @Environment(\.palette) private var palette
    let core: String
    let view: DeviceView
    let scope: ProjectInfo?

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
                                HStack(spacing: 4) {
                                    IconView(icon: Icons.folder, size: 11).foregroundStyle(palette.live)
                                    Text(projectName(call)).foregroundStyle(palette.tertiary).lineLimit(1)
                                }
                                Text(Format.age(Format.date(millis: call.raisedAt)))
                                    .monospacedDigit()
                                    .foregroundStyle(palette.warn)
                            }
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
            title: CallWords.headline(call),
            onPress: press(call),
            mark: {
                AskerMark(
                    agentId: call.asker.agentId, provider: call.asker.cli, chief: call.asker.chief == true,
                    asking: call.state == .open)
            },
            end: { HStack(spacing: 8) { end() }.font(Theme.ui(12.5)).fixedSize() }
        )
    }

    /// Opens an open call's screen to answer it, and an answered call's chat while this host still lists it.
    private func press(_ call: PersonCall) -> (() -> Void)? {
        if call.state == .open {
            return {
                HostCalls.of(core).focus(call, in: call.asker.agentId)
                Router.shared.push(.call(core: core, agent: call.asker.agentId))
            }
        }
        guard view.chats.contains(where: { $0.agentId == call.asker.agentId }) else { return nil }
        return { Router.shared.push(.chat(core: core, agent: call.asker.agentId)) }
    }

    private func projectName(_ call: PersonCall) -> String {
        view.workspace.projects.first { $0.path == call.project }?.name ?? folderName(call.project)
    }
}
