import SwiftUI

/// Where a chat runs, as the line under its composer names it.
struct ChatPlaceInfo {
    /// The host, when the account has more than one to tell apart.
    var host: String?
    var project: String?
    var branch: String?
    var goal: ChatGoal?
    /// The account the chat runs on, when its agent has more than one here.
    var account: String?
}

/// Where the chat runs, quietly under the composer: its project and branch, the host when there are several, then its
/// goal as its mark and steps, named on a tap, and its account at the right.
struct ChatPlace: View {
    @Environment(\.palette) private var palette
    let place: ChatPlaceInfo
    @State private var named = false

    var body: some View {
        HStack(spacing: 4) {
            if let host = place.host {
                part(
                    Icons.laptop, 12, palette.tertiary, Text(host).font(Theme.ui(12)).foregroundStyle(palette.tertiary)
                )
                .layoutPriority(-3)
            }
            if let project = place.project {
                part(
                    Icons.folder, 12, palette.live, Text(project).font(Theme.ui(12)).foregroundStyle(palette.secondary))
            }
            if let branch = place.branch {
                part(
                    Icons.git, 11, palette.tertiary,
                    Text(branch).font(Theme.mono(10.5)).foregroundStyle(palette.tertiary)
                )
                .layoutPriority(-1)
            }
            Spacer(minLength: 4)
            if let goal = place.goal {
                Button {
                    named.toggle()
                } label: {
                    HStack(spacing: 6) {
                        GoalStateMark(state: goal.state)
                        if named {
                            Text(goal.name).font(Theme.ui(12, .medium)).foregroundStyle(palette.ink).lineLimit(1)
                        }
                        GoalSteps(steps: goal.steps ?? [])
                    }
                    .padding(.horizontal, 4)
                    .frame(height: 24)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Goal, \(goal.name)")
            }
            if let account = place.account {
                part(
                    Icons.user, 12, palette.tertiary, Text(account).font(Theme.ui(12)).foregroundStyle(palette.tertiary)
                )
            }
        }
        .frame(height: 24)
        .padding(.top, 6)
        .padding(.horizontal, 8)
    }

    private func part(_ icon: IconDef, _ size: CGFloat, _ tint: Color, _ words: Text) -> some View {
        HStack(spacing: 6) {
            IconView(icon: icon, size: size).foregroundStyle(tint)
            words.lineLimit(1)
        }
        .padding(.horizontal, 4)
    }
}
