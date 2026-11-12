import SwiftUI

/// The faces a host's agents wear, by agent id, and the style they are drawn in.
struct Faces: Equatable {
    /// An agent the host lists without a face shows its logo; one it does not list yet wears the style's.
    let faces: [String: AgentFace?]
    let style: FaceStyle

    init?(_ view: DeviceView?) {
        guard let view, let agents = view.workspace.agents else { return nil }
        faces = Dictionary(agents.map { ($0.id, $0.face) }, uniquingKeysWith: { first, _ in first })
        style = view.faceStyle
    }

    @MainActor func face(_ agentId: String) -> AgentFace? {
        if let listed = faces[agentId] { return listed }
        guard style.on else { return nil }
        return BuddyArt.shared.styledFace(agentId: agentId, pack: style.pack, shape: style.shape)
    }
}

extension EnvironmentValues {
    @Entry var faces: Faces?
}

extension BuddyState {
    init(_ group: AgentGroup?) {
        switch group {
        case .blocked: self = .waiting
        case .working: self = .working
        case .done: self = .done
        default: self = .idle
        }
    }
}

/// The face the host gave the agent, otherwise its agent's logo.
struct AgentFaceView: View {
    @Environment(\.faces) private var faces
    let agentId: String?
    let provider: String
    var group: AgentGroup?
    /// How its turn stands, which the face wears in place of `group` where the core says it.
    var turn: TurnPhase?
    let size: CGFloat
    var logoSize: CGFloat?

    var body: some View {
        let state = turn.map { BuddyState($0) } ?? BuddyState(group)
        if let faces, let agentId, let face = faces.face(agentId), let art = BuddyArt.shared.art(face, state: state) {
            BuddyFigure(art: art, state: state, moves: faces.style.motion, size: size)
        } else {
            AgentMark(provider: provider, size: logoSize ?? size)
        }
    }
}
