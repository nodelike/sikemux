import SwiftUI
import UIKit

/// What can be done with the chat as a whole: find in it, copy a link to it, or stop it on the host.
struct ChatMenu: View {
    @Environment(\.palette) private var palette
    let core: String
    let agentId: String
    let agentName: String
    let hostName: String
    let onClose: () -> Void
    /// Absent while the chat is not open.
    var onFind: (() -> Void)?
    /// Absent while nothing was sent.
    var onRecent: (() -> Void)?
    /// Opens the snooze sheet; absent for a phone that can only watch.
    var onAway: (() -> Void)?
    /// Absent for a phone that can only watch. Answers whether the chat stopped.
    var onStop: (() async -> Bool)?
    @State private var confirming = false
    @State private var stopping = false

    var body: some View {
        if confirming, let onStop {
            VStack(alignment: .leading, spacing: 10) {
                Text("Stop this chat?").font(Theme.ui(17, .semibold)).tracking(-0.35).foregroundStyle(palette.ink).padding(.horizontal, 4)
                Text("\(agentName) stops and closes on \(hostName), along with anything it still has running.")
                    .font(Theme.ui(14)).lineSpacing(3).foregroundStyle(palette.tertiary).padding(.horizontal, 4).padding(.bottom, 6)
                ActionButton(stopping ? "Stopping…" : "Stop chat", kind: .danger, busy: stopping) {
                    stopping = true
                    Task {
                        let stopped = await onStop()
                        stopping = false
                        close()
                        if stopped { Router.shared.back() }
                    }
                }
                ActionButton("Cancel") { confirming = false }
            }
            .padding(.bottom, 4)
        } else {
            VStack(spacing: 8) {
                if let onFind { MenuItem(icon: Icons.search, label: "Find in chat") { close(); onFind() } }
                if let onRecent { MenuItem(icon: Icons.clock, label: "Recent prompts") { close(); onRecent() } }
                if let onAway { MenuItem(icon: Icons.clock, label: "Snooze") { close(); onAway() } }
                MenuItem(icon: Icons.copy, label: "Copy link") {
                    let core = core.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? core
                    let agent = agentId.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? agentId
                    UIPasteboard.general.string = "\(Config.urlScheme)://device/\(core)/chat/\(agent)"
                    Haptics.select()
                    close()
                }
                if onStop != nil { MenuItem(icon: Icons.stop, label: "Stop chat", danger: true) { confirming = true } }
            }
            .padding(.bottom, 4)
        }
    }

    private func close() {
        confirming = false
        onClose()
    }
}
