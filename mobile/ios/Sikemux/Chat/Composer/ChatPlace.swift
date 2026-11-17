import SwiftUI

/// Where a chat runs, as the line under its composer names it.
struct ChatPlaceInfo {
    /// The host, when the account has more than one to tell apart.
    var host: String?
    var project: String?
    var branch: String?
    /// The account the chat runs on, when its agent has more than one here.
    var account: String?
}

/// Where the chat runs, quietly under the composer as the Mac's line under the box: the host when there are several,
/// its project and branch with a slash between, and its account as its mark at the right, so the branch keeps its
/// whole name.
struct ChatPlace: View {
    @Environment(\.palette) private var palette
    let place: ChatPlaceInfo

    var body: some View {
        HStack(spacing: 4) {
            if let host = place.host {
                part(
                    Icons.laptop, 12, palette.tertiary, Text(host).font(Theme.ui(12)).foregroundStyle(palette.tertiary)
                )
                .layoutPriority(-3)
                if place.project != nil || place.branch != nil { slash }
            }
            if let project = place.project {
                part(
                    Icons.folder, 12, palette.live, Text(project).font(Theme.ui(12)).foregroundStyle(palette.secondary)
                )
                .layoutPriority(1)
                if place.branch != nil { slash }
            }
            if let branch = place.branch {
                part(
                    Icons.git, 11, palette.tertiary,
                    Text(branch).font(Theme.mono(10.5)).foregroundStyle(palette.tertiary)
                )
            }
            Spacer(minLength: 4)
            if let account = place.account {
                IconView(icon: Icons.user, size: 13)
                    .foregroundStyle(palette.tertiary)
                    .padding(.horizontal, 4)
                    .accessibilityLabel(String(localized: "Account, \(account)"))
            }
        }
        .frame(height: 24)
        .padding(.top, 6)
        .padding(.horizontal, 8)
    }

    private var slash: some View {
        Text("/").font(Theme.ui(13)).foregroundStyle(palette.borderStrong).accessibilityHidden(true)
    }

    private func part(_ icon: IconDef, _ size: CGFloat, _ tint: Color, _ words: Text) -> some View {
        HStack(spacing: 6) {
            IconView(icon: icon, size: size).foregroundStyle(tint)
            words.lineLimit(1)
        }
        .padding(.horizontal, 4)
    }
}
