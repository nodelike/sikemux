import SwiftUI

/// A chat someone shared, watched read-only from its link: `share/<id>#<key>`.
struct ShareScreen: View {
    let id: String
    let key: String?

    var body: some View {
        if let link = ShareLink(id: id, key: key) {
            Watching(link: link).id(link.id)
        } else {
            Screen {
                Nav("Shared chat")
                ShareProblem(icon: Icons.unlink, text: "This link is incomplete. Copy the whole link.")
            }
        }
    }
}

private struct ShareProblem: View {
    let icon: IconDef
    let text: String

    var body: some View {
        VStack {
            Spacer()
            EmptyState(icon: icon, text: text)
            Spacer()
            Spacer()
        }
        .frame(maxWidth: .infinity)
    }
}

private struct Watching: View {
    @Environment(\.palette) private var palette
    @State private var session: ShareSession
    @State private var context: TranscriptContext
    @State private var scroll = TranscriptScroll()
    @State private var since = Date()

    init(link: ShareLink) {
        let session = ShareSession(link: link)
        _session = State(initialValue: session)
        _context = State(initialValue: Self.context(session, provider: "agent"))
    }

    private static func context(_ session: ShareSession, provider: String) -> TranscriptContext {
        TranscriptContext(provider: provider, core: nil, agentId: nil, folds: Folds(), toolDetail: { [weak session] id, _ in
            guard let session else { throw CancellationError() }
            return try await session.toolDetail(id)
        })
    }

    private var provider: String { session.chat?.provider ?? "agent" }

    private var problem: (icon: IconDef, text: String)? {
        switch session.status {
        case .ended:
            session.endedBecause == .expired ? (Icons.hourglass, "This share has expired.") : (Icons.stop, "Sharing this chat has stopped.")
        case .missing: (Icons.power, "This chat is not running on its host right now.")
        case .notFound: (Icons.unlink, "No chat is shared at this link.")
        case .signedOut: (Icons.user, "Sign in to Sikemux to watch shared chats.")
        case .unreadable: (Icons.unlink, "This link does not open the chat it points at. Check that the whole link was copied.")
        case .connecting, .live: nil
        }
    }

    private var footer: String {
        if let problem { return problem.text }
        guard session.hostOnline else { return "Read-only · the sharer is offline" }
        guard let ends = session.expiresAt else { return "Read-only" }
        return "Read-only · link ends in \(Format.timeLeft(until: ends))"
    }

    var body: some View {
        let rows = session.rows
        Screen {
            Nav(back: nil, onBack: nil, title: {
                HStack(spacing: 8) {
                    AgentMark(provider: provider, size: 17)
                    Text(session.title).font(Theme.ui(16, .semibold)).tracking(-0.25).foregroundStyle(palette.ink).lineLimit(1)
                        .accessibilityAddTraits(.isHeader)
                }
            }, end: { EmptyView() })
            if rows.messages.isEmpty {
                if let problem {
                    ShareProblem(icon: problem.icon, text: problem.text)
                } else {
                    VStack {
                        Spacer()
                        HStack(spacing: 10) {
                            Working()
                            Text("Opening the shared chat…").metaStyle(palette)
                        }
                        .padding(.vertical, 24)
                        .accessibilityElement(children: .combine)
                    }
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                }
            } else {
                Transcript(
                    rows: rows, drawn: session.drawn, touched: { session.takeTouched() }, context: context, scroll: scroll,
                    hasEarlier: session.hasEarlier, earlier: session.earlier, loadEarlier: { session.loadEarlier() }
                ) {
                    VStack(alignment: .leading, spacing: 0) {
                        ForEach(rows.permissions, id: \.requestId) { request in
                            Text("Waiting for permission: \(request.toolCall.title)")
                                .font(Theme.ui(13.5)).foregroundStyle(palette.tertiary).padding(.top, 10)
                        }
                        if session.status == .live, let activity = ChatWords.activity(rows.head.activity) {
                            ActivityLine(provider: provider, label: activity, since: since)
                        }
                        if let error = rows.head.error {
                            Text(error).font(Theme.ui(13.5)).foregroundStyle(palette.danger).padding(.top, 10)
                        }
                    }
                    .padding(.bottom, 28)
                }
                .overlay(alignment: .bottom) { JumpToLatest(scroll: scroll) }
            }
            HStack(spacing: 10) {
                IconView(icon: Icons.eye, size: 17).foregroundStyle(palette.inkDim)
                Text(footer).font(Theme.ui(13.5)).lineSpacing(5.5).foregroundStyle(palette.inkDim)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .padding(14)
            .background(RoundedRectangle(cornerRadius: 16).fill(palette.composer))
            .overlay(RoundedRectangle(cornerRadius: 16).strokeBorder(palette.border, lineWidth: 1))
            .padding(.horizontal, 10)
            .padding(.top, 8)
            .padding(.bottom, 6)
        }
        .onAppear { session.start() }
        .onDisappear { session.stop() }
        .onChange(of: rows.subagentRows) { _, now in context.subagentRows = now }
        .onChange(of: provider) { _, now in
            context = Self.context(session, provider: now)
            context.subagentRows = rows.subagentRows
        }
    }
}
