import SwiftUI

/// The quiet line an Overrule leaves in the chief's chat, as the Mac words it: what was decided, quoted, and Undo until
/// it is undone, or when it stopped Auto's run, which Undo cannot put back.
struct OverruledLine: View {
    @Environment(\.palette) private var palette
    @Environment(TranscriptContext.self) private var context
    let overruled: OverruledDecision
    @State private var sending = false
    @State private var refused: String?

    private var why: String? {
        if overruled.undone == true { return CallWords.standsAgain }
        if overruled.stoppedAuto == true { return CallWords.stoppedAuto }
        return overruled.rule != nil ? CallWords.fromNowOn : nil
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 8) {
                IconView(icon: Icons.reply, size: 12).foregroundStyle(palette.tertiary)
                Text(overruled.undone == true ? CallWords.overruledUndone : CallWords.overruled)
                    .foregroundStyle(palette.tertiary)
                Spacer(minLength: 12)
                if context.acts && overruled.undone != true && overruled.stoppedAuto != true {
                    Button(CallWords.undo, action: undo)
                        .buttonStyle(.plain)
                        .font(Theme.ui(13, .medium))
                        .foregroundStyle(palette.ink)
                        .opacity(sending ? 0.5 : 1)
                        .disabled(sending)
                }
            }
            .font(Theme.ui(13))
            if let text = overruled.text, !text.isEmpty {
                Text(CallWords.quoted(text)).font(Theme.ui(14)).foregroundStyle(palette.ink).lineLimit(2)
            }
            if let why { Text(why).font(Theme.ui(12.5)).foregroundStyle(palette.tertiary) }
            if let refused { Text(refused).font(Theme.ui(12.5)).foregroundStyle(palette.danger) }
        }
        .padding(.leading, 12)
        .padding(.vertical, 10)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func undo() {
        guard let core = context.core, let connection = Hub.shared.live(core).connection, !sending else { return }
        sending = true
        refused = nil
        let params = UndoOverruleParams(project: overruled.project, goal: overruled.goal, decision: overruled.decision)
        Task {
            defer { sending = false }
            do {
                _ = try await connection.call(PhoneCall.UndoOverrule.self, params)
            } catch {
                refused = problem(error)
            }
        }
    }
}

/// The chief's count of last week, in its own words under the day and time it came.
struct WeekLine: View {
    @Environment(\.palette) private var palette
    let week: ChiefWeek

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(CallWords.weekCame(Date(timeIntervalSince1970: Double(week.to) / 1000)))
                .font(Theme.ui(12))
                .foregroundStyle(palette.tertiary)
            Text(CallWords.weekCount(week)).font(Theme.ui(14.5)).foregroundStyle(palette.ink)
        }
        .padding(.vertical, 12)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}
