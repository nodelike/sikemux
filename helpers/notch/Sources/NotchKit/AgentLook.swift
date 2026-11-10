import SwiftUI

extension BuddyState {
    init(_ mark: Mark) {
        switch mark {
        case .needs: self = .waiting
        case .working: self = .working
        case .done: self = .done
        case .failed: self = .error
        case .idle: self = .idle
        }
    }
}

/// An agent's face, when the core gives it one, otherwise its agent's mark.
struct AgentLook: View {
    let face: AgentFace?
    let provider: String
    var mark: Mark = .idle
    var size: CGFloat = 16
    @Environment(\.faceMotion) private var moves

    var body: some View {
        if let face, let art = BuddyArt.shared.art(face, state: BuddyState(mark)) {
            BuddyFigure(art: art, state: BuddyState(mark), moves: moves, size: size)
        } else {
            AgentMark(provider: provider, size: size)
        }
    }
}

extension AgentLook {
    init(entry: Entry, size: CGFloat) {
        self.init(face: entry.agent?.face, provider: entry.provider, mark: entry.mark, size: size)
    }
}
