import SwiftUI

/// An agent's ask to sign in to connectors again, a row for each, which folds to one line once every one is back or it
/// was dismissed. The sign-in itself runs on the Mac.
struct SignInNotice: View {
    @Environment(\.palette) private var palette
    @Environment(TranscriptContext.self) private var context
    let card: SignInCard
    @State private var sending = false
    @State private var refused: String?

    private var left: [SignInConnector] { card.connectors.filter { $0.state != .signedIn } }

    var body: some View {
        if left.isEmpty {
            NoticeLine {
                HStack(spacing: 8) {
                    IconView(icon: Icons.check, size: 12).foregroundStyle(palette.live)
                    Text("Signed in to \(Self.names(card.connectors))").foregroundStyle(palette.ink).lineLimit(1)
                    Text("· @\(card.name) carried on").foregroundStyle(palette.inkDim).lineLimit(1).fixedSize()
                }
                .font(Theme.ui(12.5))
                .frame(minHeight: 18)
            }
            .accessibilityElement(children: .combine)
        } else if card.skipped == true {
            NoticeLine {
                HStack(spacing: 8) {
                    IconView(icon: Icons.lock, size: 12)
                    Text("Did not sign in to \(Self.names(left))").font(Theme.ui(12.5)).lineLimit(1)
                }
                .foregroundStyle(palette.inkDim)
                .frame(minHeight: 18)
            }
            .accessibilityElement(children: .combine)
        } else {
            open
        }
    }

    private var open: some View {
        let shape = RoundedRectangle(cornerRadius: 10)
        return VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                IconView(icon: Icons.lock, size: 12).foregroundStyle(palette.inkDim)
                Text("@\(card.name) needs you signed in").foregroundStyle(palette.ink).lineLimit(1).fixedSize()
                Text(card.why).foregroundStyle(palette.inkDim).lineLimit(1)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .layoutPriority(-1)
                if context.acts {
                    Button(action: dismiss) {
                        IconView(icon: Icons.close, size: 12).foregroundStyle(palette.inkDim)
                            .frame(width: 24, height: 24)
                            .contentShape(Rectangle().inset(by: -10))
                    }
                    .buttonStyle(ActiveFill(radius: 6))
                    .opacity(sending ? 0.5 : 1)
                    .disabled(sending)
                    .accessibilityLabel("Dismiss")
                }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
            if let refused {
                Text(refused).foregroundStyle(palette.danger).padding(.horizontal, 12).padding(.bottom, 8)
            }
            ForEach(card.connectors, id: \.id) { connector in
                Rectangle().fill(palette.border).frame(height: 1)
                SignInRow(connector: connector)
            }
        }
        .font(Theme.ui(12.5))
        .background(shape.fill(palette.raised))
        .overlay(shape.strokeBorder(palette.border, lineWidth: 1))
        .padding(.vertical, 6)
    }

    private static func names(_ connectors: [SignInConnector]) -> String {
        connectors.map(\.name).joined(separator: ", ")
    }

    private func dismiss() {
        guard !sending else { return }
        guard let core = context.core, let connection = Hub.shared.live(core).connection else {
            refused = CallWords.notConnected
            return
        }
        Haptics.select()
        sending = true
        refused = nil
        Task {
            defer { sending = false }
            do {
                _ = try await connection.call(PhoneCall.DismissSignIn.self, SignInCardParams(id: card.id))
            } catch {
                refused = problem(error)
            }
        }
    }
}

private struct SignInRow: View {
    @Environment(\.palette) private var palette
    let connector: SignInConnector

    /// The connector's mark, by its id, as `sikemux.jira` is `IconJira`.
    private var mark: IconDef {
        let name = connector.id.replacing("sikemux.", with: "", maxReplacements: 1)
        return Icons.named("Icon\(name.prefix(1).uppercased())\(name.dropFirst())") ?? Icons.plug
    }

    var body: some View {
        let tile = RoundedRectangle(cornerRadius: 6)
        HStack(spacing: 12) {
            IconView(icon: mark, size: 16).foregroundStyle(palette.secondary)
                .frame(width: 28, height: 28)
                .background(tile.fill(palette.active))
                .overlay(tile.strokeBorder(palette.border, lineWidth: 1))
            VStack(alignment: .leading, spacing: 0) {
                Text(connector.name).foregroundStyle(palette.ink).lineLimit(1)
                line
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            if connector.state == .signedIn {
                IconView(icon: Icons.check, size: 14).foregroundStyle(palette.live)
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .accessibilityElement(children: .combine)
    }

    @ViewBuilder private var line: some View {
        switch connector.state {
        case .signedIn: Text("Signed in").foregroundStyle(palette.inkDim)
        case .failed(let failed): Text(failed.reason).foregroundStyle(palette.danger).lineLimit(1)
        case .waiting, .unknown: Text("Not signed in").foregroundStyle(palette.inkDim)
        }
    }
}
