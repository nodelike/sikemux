import SwiftUI

/// What every row of a transcript reads besides its message: whose chat it is, which folds are open, how to read more
/// from where it comes from, and what a tap asks of the screen around it.
@MainActor @Observable
final class TranscriptContext {
    let provider: String
    /// The host the chat is on, which keeps the files it names; nil where none can be read.
    let core: String?
    let agentId: String?
    let folds: Folds
    /// Reads a call's whole change and what it printed; nil where nothing can read it.
    @ObservationIgnored let toolDetail: ((String, Bool) async throws -> ToolDetail)?
    /// Each subagent's own rows, by its session.
    var subagentRows: [String: [ChatMessage]] = [:]
    /// The picture shown over the transcript.
    var picture: ShownPicture?
    /// What the find bar found, while it is open.
    var finding: Finding?
    @ObservationIgnored let actions = TranscriptActions()

    init(provider: String, core: String?, agentId: String?, folds: Folds, toolDetail: ((String, Bool) async throws -> ToolDetail)?) {
        self.provider = provider
        self.core = core
        self.agentId = agentId
        self.folds = folds
        self.toolDetail = toolDetail
    }

    func showImage(_ image: UIImage, _ name: String) {
        picture = ShownPicture(image: image, name: name)
    }

    /// The card of the chat's subagent `sessionId`, as its rows carry it.
    func subagent(_ sessionId: String, in messages: [ChatMessage]) -> SubagentCard? {
        for rows in [messages] + Array(subagentRows.values) {
            for row in rows.reversed() {
                for part in row.parts {
                    if case .subagent(let card) = part, card.subagent.sessionId == sessionId { return card.subagent }
                }
            }
        }
        return nil
    }
}

struct ShownPicture: Identifiable {
    let id = UUID()
    let image: UIImage
    let name: String
}

/// A live label with the Mac's sweep: a bright band slides across text drawn in its own colour.
struct Shimmer: View {
    @Environment(\.palette) private var palette
    @Environment(\.accessibilityReduceMotion) private var still
    let text: String
    let font: Font
    let color: Color
    @State private var moving = false

    var body: some View {
        let label = Text(text).font(font).lineLimit(1)
        if still {
            label.foregroundStyle(color)
        } else {
            label.foregroundStyle(color)
                .overlay {
                    GeometryReader { size in
                        let band = max(48, size.size.width * 0.5)
                        LinearGradient(colors: [.clear, palette.ink.opacity(0.9), .clear], startPoint: .leading, endPoint: .trailing)
                            .frame(width: band)
                            .offset(x: moving ? size.size.width : -band)
                    }
                    .mask(label)
                    .allowsHitTesting(false)
                }
                .onAppear {
                    withAnimation(.linear(duration: 2.2).repeatForever(autoreverses: false)) { moving = true }
                }
        }
    }
}
