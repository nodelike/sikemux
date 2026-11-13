import SwiftUI

/// A decision's quiet line in a chat: what became of it with its one action, the decision quoted, then why. The decided
/// line and the Overruled line it turns into share it.
private struct DecisionLineLayout: View {
    @Environment(\.palette) private var palette
    let icon: IconDef
    let head: String
    let action: String?
    let sending: Bool
    let act: () -> Void
    let quote: String?
    let why: String?
    let refused: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 8) {
                IconView(icon: icon, size: 12).foregroundStyle(palette.tertiary)
                Text(head).foregroundStyle(palette.tertiary)
                Spacer(minLength: 12)
                if let action {
                    Button(action, action: act)
                        .buttonStyle(.plain)
                        .font(Theme.ui(13, .medium))
                        .foregroundStyle(palette.ink)
                        .opacity(sending ? 0.5 : 1)
                        .disabled(sending)
                }
            }
            .font(Theme.ui(13))
            if let quote, !quote.isEmpty {
                Text(CallWords.quoted(quote)).font(Theme.ui(14)).foregroundStyle(palette.ink).lineLimit(2)
            }
            if let why { Text(why).font(Theme.ui(12.5)).foregroundStyle(palette.tertiary).lineLimit(2) }
            if let refused { Text(refused).font(Theme.ui(12.5)).foregroundStyle(palette.danger) }
        }
        .padding(.leading, 12)
        .padding(.vertical, 10)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// The line under an agent's reply that says it recorded a decision itself, as the Mac words it: Decided, with Overrule
/// on a phone with full access, the decision quoted and what it rests on.
struct AgentDecidedLine: View {
    @Environment(TranscriptContext.self) private var context
    let decided: DecidedLine
    @State private var sending = false
    @State private var refused: String?

    var body: some View {
        DecisionLineLayout(
            icon: Icons.check, head: CallWords.decided, action: context.acts ? CallWords.overrule : nil,
            sending: sending, act: overrule, quote: decided.text, why: decided.why, refused: refused)
    }

    private func overrule() {
        guard !sending else { return }
        guard let core = context.core, let connection = Hub.shared.live(core).connection else {
            refused = CallWords.notConnected
            return
        }
        sending = true
        refused = nil
        let params = OverruleParams(project: decided.project, goal: decided.goal, decision: decided.decision)
        Task {
            defer { sending = false }
            do {
                _ = try await connection.call(PhoneCall.OverruleDecision.self, params)
            } catch {
                refused = problem(error)
            }
        }
    }
}

/// The quiet line an Overrule leaves in the chief's chat, as the Mac words it: what was decided, quoted, and Undo until
/// it is undone, or when it stopped Auto's run, which Undo cannot put back.
struct OverruledLine: View {
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
        let undoes = context.acts && overruled.undone != true && overruled.stoppedAuto != true
        DecisionLineLayout(
            icon: Icons.reply, head: overruled.undone == true ? CallWords.overruledUndone : CallWords.overruled,
            action: undoes ? CallWords.undo : nil, sending: sending, act: undo, quote: overruled.text, why: why,
            refused: refused)
    }

    private func undo() {
        guard !sending else { return }
        guard let core = context.core, let connection = Hub.shared.live(core).connection else {
            refused = CallWords.notConnected
            return
        }
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

/// The quiet line a rule change leaves, as the Mac words it on one line: its title, the rule quoted, which gives way
/// first, then the check's short hash or who did it. The phone offers no Undo on it.
struct RuleLine: View {
    @Environment(\.palette) private var palette
    let line: RuleChange

    var body: some View {
        let (head, tail) = CallWords.ruleChange(line)
        let rule = CallWords.quoted(line.text)
        HStack(spacing: 8) {
            IconView(icon: Icons.check, size: 12).foregroundStyle(palette.tertiary)
            Text(head).foregroundStyle(palette.tertiary).fixedSize()
            Text("· \(rule)").foregroundStyle(palette.ink).lineLimit(1)
            if let tail { Text("· \(tail)").foregroundStyle(palette.tertiary).fixedSize() }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel([head, rule, tail].compactMap { $0 }.joined(separator: ", "))
        .font(Theme.ui(13))
        .padding(.leading, 12)
        .padding(.vertical, 6)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}
