import SwiftUI

/// Calls folded to a line each, as the Mac's chat folds them: the asker's mark, the question and how long it has
/// waited. Two or more fold under one line that names the newest, and a tap on it opens their lines.
struct CallRows: View {
    @Environment(\.palette) private var palette
    let calls: [PersonCall]
    /// What the fold's line says of how many calls it holds.
    let counted: (Int) -> String
    /// Each line ends on a chevron, as it opens a screen of its own.
    var opens = false
    let onPress: (PersonCall) -> Void
    @State private var open = false

    private static let row: CGFloat = 44

    var body: some View {
        VStack(spacing: 0) {
            if calls.count > 1, let newest = calls.max(by: { $0.raisedAt < $1.raisedAt }) {
                Button {
                    Haptics.select()
                    open.toggle()
                } label: {
                    HStack(spacing: 10) {
                        mark(newest)
                        (Text(counted(calls.count)).foregroundStyle(palette.ink)
                            + Text(" · ").foregroundStyle(palette.inkFaint)
                            + Text(CallWords.newest(CallWords.headline(newest))).foregroundStyle(palette.secondary))
                            .font(Theme.ui(14)).lineLimit(1)
                        Spacer(minLength: 8)
                        age(newest)
                        IconView(icon: Icons.chevron, size: 11).foregroundStyle(palette.tertiary)
                            .rotationEffect(.degrees(open ? 90 : 0))
                    }
                    .frame(minHeight: Self.row)
                    .contentShape(Rectangle())
                }
                .buttonStyle(PressFade())
                .accessibilityValue(open ? "expanded" : "collapsed")
            }
            if calls.count == 1 || open {
                ForEach(calls, id: \.id) { call in
                    Button {
                        Haptics.select()
                        onPress(call)
                    } label: {
                        HStack(spacing: 10) {
                            mark(call)
                            Text(CallWords.headline(call)).font(Theme.ui(14))
                                .foregroundStyle(opens ? palette.ink : palette.secondary).lineLimit(1)
                            Spacer(minLength: 8)
                            age(call)
                            if opens {
                                IconView(icon: Icons.chevron, size: 11).foregroundStyle(palette.tertiary)
                            }
                        }
                        .frame(minHeight: Self.row)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(PressFade())
                }
            }
        }
    }

    private func mark(_ call: PersonCall) -> some View {
        AskerMark(
            agentId: call.asker.agentId, provider: call.asker.cli, chief: call.asker.chief == true, asking: true,
            size: 16)
    }

    private func age(_ call: PersonCall) -> some View {
        Text(Format.age(Format.date(millis: call.raisedAt))).font(Theme.ui(12.5)).foregroundStyle(palette.warn)
    }
}
