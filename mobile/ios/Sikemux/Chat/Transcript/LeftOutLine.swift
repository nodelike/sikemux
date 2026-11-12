import SwiftUI
import os

private let log = Logger(subsystem: "com.nodelike.sikemux", category: "left-out")

/// The quiet line under an answer some of whose sentences rested on no source and were left out, as the Mac says it.
/// Show asks the host for the answer's parts with those sentences back where they stood, drawn faint; Hide takes them
/// out again. When the host cannot put them back (an answer too big for one page, a refusal, no connection, or no host
/// as in a shared chat), Show lists them under the answer, faint. The words are the Android app's shared/chat/leftOut.ts.
struct LeftOutLine: View {
    @Environment(\.palette) private var palette
    @Environment(TranscriptContext.self) private var context
    let message: ChatMessage
    @State private var asking = false

    private var count: Int { (message.leftOut ?? []).reduce(0) { $0 + Int($1.sentences) } }
    private var shown: Bool { context.leftOut[message.id] != nil }

    var body: some View {
        if count > 0 {
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 6) {
                    Text(Self.line(count))
                    Text("·")
                    Button(shown ? String(localized: "Hide") : String(localized: "Show"), action: toggle)
                        .buttonStyle(.plain)
                        .foregroundStyle(palette.secondary)
                        .accessibilityValue(shown ? "Shown" : "")
                }
                .foregroundStyle(palette.tertiary)
                if context.leftOut[message.id]?.isEmpty == true {
                    ForEach(Array((message.leftOut ?? []).enumerated()), id: \.offset) { _, out in
                        Text(out.text.trimmingCharacters(in: .whitespacesAndNewlines))
                            .font(Theme.ui(14.5))
                            .foregroundStyle(palette.tertiary)
                            .padding(.top, 4)
                    }
                }
            }
            .font(Theme.ui(12.5))
            .padding(.top, 6)
        }
    }

    static func line(_ count: Int) -> String {
        count == 1
            ? String(localized: "1 sentence with no source left out")
            : String(localized: "\(count) sentences with no source left out")
    }

    private func toggle() {
        if shown {
            context.leftOut[message.id] = nil
            return
        }
        guard !asking else { return }
        guard let core = context.core, let agentId = context.agentId, let connection = Hub.shared.live(core).connection
        else {
            context.leftOut[message.id] = [:]
            return
        }
        asking = true
        let params = ChatLeftOutParams(agentId: agentId, messageId: message.id)
        Task {
            defer { asking = false }
            do {
                let answer = try await connection.call(PhoneCall.ChatLeftOut.self, params)
                context.leftOut[message.id] = Dictionary(
                    answer.parts.map { ($0.part, $0.md) }, uniquingKeysWith: { first, _ in first })
            } catch {
                log.error("left out: \(problem(error), privacy: .public)")
                context.leftOut[message.id] = [:]
            }
        }
    }
}
