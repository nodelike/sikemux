import SwiftUI

/// An agent's mark where a call or a row names it: the project's chief wears Sikemux's S in the accent, any other
/// agent its own face.
struct AskerMark: View {
    @Environment(\.palette) private var palette
    let agentId: String
    let provider: String
    var chief = false
    var turn: TurnState?
    var asking = false
    var size: CGFloat = 20

    var body: some View {
        if chief {
            IconView(icon: Icons.brand, size: size * 0.85)
                .foregroundStyle(palette.accent)
                .frame(width: size, height: size)
        } else {
            AgentFaceView(agentId: agentId, provider: provider, turn: turn, asking: asking, size: size)
                .frame(width: size, height: size)
        }
    }
}
