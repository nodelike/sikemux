import SwiftUI

/// What stands at the end of the chat when something went wrong, as on the Mac: that the session dropped, or why the
/// last turn failed, with what can be done about it from here. Signing in happens on the host.
struct Trouble: View {
    @Environment(\.palette) private var palette
    let session: ChatSession
    let provider: String
    let hostName: String
    let canAct: Bool
    @State private var accounts: ChatAccounts?
    @State private var now = Date()

    private var label: String { Catalog.label(provider, core: session.core) }
    private var maker: String? { Catalog.entry(provider, core: session.core)?.maker }

    var body: some View {
        let head = session.rows.head
        let failure = head.failure
        let dropped = head.connection == .error || head.connection == .stopped
        Group {
            if dropped && !session.rows.messages.isEmpty {
                VStack(alignment: .leading, spacing: 8) {
                    said(
                        Icons.plug, head.ended?.message.map { "This session dropped: \($0)" } ?? "This session dropped."
                    )
                    if canAct {
                        actions {
                            action("Reconnect") { session.retry() }
                        }
                    }
                }
                .padding(.top, 10)
            } else if let error = head.error {
                let network = failure?.kind == .network
                let signIn = failure?.kind == .signIn
                let resumes =
                    failure?.kind == .limit
                    && (failure?.resumesAt).map { Double($0) / 1000 > now.timeIntervalSince1970 } == true
                let moves = failure?.kind == .limit ? accounts?.others ?? [] : []
                VStack(alignment: .leading, spacing: 8) {
                    said(
                        Icons.warning,
                        network
                            ? "Lost connection to \(maker ?? "the model provider"). Sikemux sends it again when you're back online."
                            : error)
                    if network, let details = failure?.details {
                        Text(details).font(Theme.mono(11.5)).lineSpacing(3).foregroundStyle(palette.tertiary)
                            .textSelection(.enabled).padding(.leading, 22)
                    }
                    if signIn {
                        Text("Sign in to \(accounts?.account?.label ?? label) on \(hostName), then retry here.")
                            .font(Theme.ui(13)).lineSpacing(3).foregroundStyle(palette.tertiary).padding(.leading, 22)
                    }
                    if canAct && (network || resumes || !moves.isEmpty || signIn) {
                        actions {
                            if network {
                                action("Retry now") { session.failedTurn(.retry) }
                                action("Discard") { session.failedTurn(.discard) }
                            }
                            if resumes { action("Don't resume") { session.failedTurn(.cancelResume) } }
                            ForEach(moves, id: \.id) { other in
                                action("Continue on \(other.label)") { session.moveToAccount(other.id) }
                            }
                            if signIn { action("Retry") { session.failedTurn(.retry) } }
                        }
                    }
                }
                .padding(.top, 10)
                .task(id: "\(canAct)|\(failure?.kind.rawValue ?? "")|\(failure?.account ?? "")") {
                    await readAccounts(failure)
                }
                .task(id: failure?.resumesAt) {
                    guard let at = failure?.resumesAt else { return }
                    let wait = Double(at) / 1000 - Date().timeIntervalSince1970
                    if wait > 0 { try? await Task.sleep(for: .seconds(wait)) }
                    now = Date()
                }
            }
        }
    }

    private func readAccounts(_ failure: ChatFailure?) async {
        guard canAct, failure?.kind == .signIn || failure?.kind == .limit else { return }
        do {
            accounts = try await session.openConnection().call(
                PhoneCall.ChatAccounts.self, ChatAccountsParams(agentId: session.agentId))
        } catch {
            print("sikemux: could not read the chat's accounts: \(error)")
        }
    }

    private func said(_ icon: IconDef, _ text: String) -> some View {
        HStack(alignment: .top, spacing: 8) {
            IconView(icon: icon, size: 14).foregroundStyle(palette.danger).frame(height: 19)
            Text(text).font(Theme.ui(13.5)).lineSpacing(19 - 13.5 * 1.22).foregroundStyle(palette.secondary)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    private func actions<Content: View>(@ViewBuilder _ content: () -> Content) -> some View {
        FlowRow(spacing: 8) { content() }.padding(.leading, 22)
    }

    private func action(_ title: String, _ run: @escaping () -> Void) -> some View {
        Button(action: run) {
            Text(title).font(Theme.ui(13, .semibold)).foregroundStyle(palette.ink)
                .padding(.horizontal, 13).frame(height: 32)
                .overlay(Capsule().strokeBorder(palette.border, lineWidth: 1))
                .contentShape(Capsule())
        }
        .buttonStyle(ActiveFill(radius: 16))
    }
}

/// Lays its children out in rows, wrapping to the next when one is full.
struct FlowRow: Layout {
    var spacing: CGFloat = 8

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let rows = rows(proposal.width ?? .infinity, subviews)
        let height = rows.map(\.height).reduce(0, +) + spacing * CGFloat(max(0, rows.count - 1))
        return CGSize(width: proposal.width ?? rows.map(\.width).max() ?? 0, height: height)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        var y = bounds.minY
        for row in rows(bounds.width, subviews) {
            var x = bounds.minX
            for index in row.items {
                let size = subviews[index].sizeThatFits(.unspecified)
                subviews[index].place(at: CGPoint(x: x, y: y), proposal: ProposedViewSize(size))
                x += size.width + spacing
            }
            y += row.height + spacing
        }
    }

    private func rows(_ width: CGFloat, _ subviews: Subviews) -> [(items: [Int], width: CGFloat, height: CGFloat)] {
        var rows: [(items: [Int], width: CGFloat, height: CGFloat)] = []
        var current: (items: [Int], width: CGFloat, height: CGFloat) = ([], 0, 0)
        for index in subviews.indices {
            let size = subviews[index].sizeThatFits(.unspecified)
            let next = current.items.isEmpty ? size.width : current.width + spacing + size.width
            if next > width, !current.items.isEmpty {
                rows.append(current)
                current = ([index], size.width, size.height)
            } else {
                current = (current.items + [index], next, max(current.height, size.height))
            }
        }
        if !current.items.isEmpty { rows.append(current) }
        return rows
    }
}
