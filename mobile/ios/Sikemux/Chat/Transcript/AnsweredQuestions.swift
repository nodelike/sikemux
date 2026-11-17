import SwiftUI

/// One question an agent asked and how the person answered it.
struct Answered {
    var question: String
    var answer: String
    var secret: Bool
    var files: [String] = []
}

/// The questions an agent asked and the person's answers, as the conversation they were: each question in the asker's
/// bubble at the left, its mark on the first, and the answer in the person's bubble at the right. Someone else's
/// questions say who asked above them.
struct AnsweredQuestions: View {
    @Environment(\.palette) private var palette
    let agentId: String
    let cli: String
    /// Who asked, when it was not this chat's own agent.
    let asker: String?
    let answers: [Answered]

    private static let mark: CGFloat = 16

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            if let asker {
                Text("@\(asker)").font(Theme.ui(12)).foregroundStyle(palette.tertiary).padding(.leading, Self.mark + 10)
            }
            ForEach(Array(answers.enumerated()), id: \.offset) { index, answered in
                VStack(alignment: .leading, spacing: 6) {
                    HStack(alignment: .top, spacing: 10) {
                        Group {
                            if index == 0 {
                                AgentFaceView(agentId: agentId, provider: cli, size: Self.mark).padding(.top, 13)
                            } else {
                                Color.clear.frame(height: 0)
                            }
                        }
                        .frame(width: Self.mark)
                        Text(answered.question).sentWords(palette).textSelection(.enabled)
                            .fixedSize(horizontal: false, vertical: true)
                            .theirBubble(palette)
                    }
                    .sentWidth(0.86, alignment: .leading)
                    VStack(alignment: .trailing, spacing: 6) {
                        if !answered.files.isEmpty { SentAttachments(paths: answered.files) }
                        if answered.secret {
                            Text("Kept private").font(Theme.ui(14)).foregroundStyle(palette.tertiary)
                                .mineBubble(palette)
                        } else if !answered.answer.isEmpty {
                            Text(answered.answer).sentWords(palette).textSelection(.enabled)
                                .fixedSize(horizontal: false, vertical: true)
                                .mineBubble(palette)
                        }
                    }
                    .sentWidth(0.86)
                    .frame(maxWidth: .infinity, alignment: .trailing)
                }
            }
        }
        .padding(.top, 14)
        .padding(.bottom, 6)
    }
}
