import SwiftUI

/// What every row of a transcript reads besides its message: whose chat it is, which folds are open, how to read more
/// from where it comes from, and what a tap asks of the screen around it.
@MainActor @Observable
final class TranscriptContext {
    /// Whose marks the rows draw; a chat just started learns it once the host lists the chat.
    var provider: String
    /// The host the chat is on, which keeps the files it names; nil where none can be read.
    let core: String?
    let agentId: String?
    let folds: Folds
    /// Reads a call's whole change and what it printed; nil where nothing can read it.
    @ObservationIgnored let toolDetail: ((String, Bool) async throws -> ToolDetail)?
    /// The chat's own rows, read where they are kept so a view reading them redraws when they change.
    @ObservationIgnored let messages: () -> [ChatMessage]
    /// Each subagent's own rows, by its session.
    var subagentRows: [String: [ChatMessage]] = [:]
    /// The picture shown over the transcript.
    var picture: ShownPicture?
    /// What the find bar found, while it is open.
    var finding: Finding?
    @ObservationIgnored let actions = TranscriptActions()
    @ObservationIgnored let reveal = Reveal()

    init(
        provider: String, core: String?, agentId: String?, folds: Folds, messages: @escaping () -> [ChatMessage],
        toolDetail: ((String, Bool) async throws -> ToolDetail)?
    ) {
        self.messages = messages
        self.provider = provider
        self.core = core
        self.agentId = agentId
        self.folds = folds
        self.toolDetail = toolDetail
    }

    func showImage(_ image: UIImage, _ name: String) {
        picture = ShownPicture(image: image, name: name)
    }

    /// Opens the viewer on the host's picture `path` at once, and fills it when the phone has fetched it.
    func showHostImage(core: String, path: String) {
        let shown = ShownPicture(name: (path as NSString).lastPathComponent)
        picture = shown
        Task {
            let file = await HostFiles.shared.read(core: core, path: path)
            guard picture?.id == shown.id else { return }
            if let file, let image = UIImage(contentsOfFile: file.path) {
                picture?.image = image
            } else {
                Haptics.failure()
                picture?.problem = "Could not get it from the host."
            }
        }
    }

    /// The card of the chat's subagent `sessionId`, as its rows carry it.
    func subagent(_ sessionId: String) -> SubagentCard? {
        for rows in [messages()] + Array(subagentRows.values) {
            for row in rows.reversed() {
                for part in row.parts {
                    if case .subagent(let card) = part, card.subagent.sessionId == sessionId { return card.subagent }
                }
            }
        }
        return nil
    }
}

/// A picture over the transcript: its image once the phone has it, or why it could not get it.
struct ShownPicture: Identifiable {
    let id = UUID()
    var image: UIImage?
    let name: String
    var problem: String?
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
                        LinearGradient(
                            colors: [.clear, palette.ink.opacity(0.9), .clear], startPoint: .leading,
                            endPoint: .trailing
                        )
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
