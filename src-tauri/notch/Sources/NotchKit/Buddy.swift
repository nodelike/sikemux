import AppKit
import JavaScriptCore
import SwiftUI

enum BuddyState: String {
    case idle, working, waiting, done, error

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

/// Faces drawn by the app's own script, so the island's are the ones the app shows.
@MainActor
final class BuddyArt {
    static let shared = BuddyArt()

    private lazy var draw: JSValue? = {
        let context = JSContext()
        context?.evaluateScript(buddiesScript)
        return context?.objectForKeyedSubscript("SikemuxBuddies")?.objectForKeyedSubscript("notchFace")
    }()
    private var images: [String: NSImage] = [:]

    /// The face the core gave an agent, drawn in `state`.
    func image(_ face: AgentFace, state: BuddyState) -> NSImage? {
        let key = "\(face.pack) \(face.shape) \(face.color) \(face.box) \(state.rawValue)"
        if let image = images[key] { return image }
        let described: [String: Any] = ["pack": face.pack, "shape": face.shape, "color": face.color, "box": face.box]
        guard let drawn = draw?.call(withArguments: [described, state.rawValue]), !drawn.isNull,
              let svg = drawn.toString(),
              let image = NSImage(data: Data(svg.utf8))
        else { return nil }
        images[key] = image
        return image
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
        if let face, let image = BuddyArt.shared.image(face, state: BuddyState(mark)) {
            BuddyImage(image: image, state: BuddyState(mark), moves: moves, size: size)
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

/// The app's bob, hop and shake, which move the whole buddy by units of its drawing.
private struct BuddyImage: View {
    let image: NSImage
    let state: BuddyState
    let moves: Bool
    let size: CGFloat
    @Environment(\.stillMarks) private var still
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        let art = Image(nsImage: image).resizable().interpolation(.high).frame(width: size, height: size)
        let unit = size / max(image.size.width, 1)
        if moves, !still, !reduceMotion, state == .working || state == .waiting || state == .error {
            art.keyframeAnimator(initialValue: CGFloat(0), repeating: true) { content, offset in
                state == .error ? content.offset(x: offset * unit) : content.offset(y: offset * unit)
            } keyframes: { _ in
                switch state {
                case .working:
                    CubicKeyframe(-0.8, duration: 0.7)
                    CubicKeyframe(0, duration: 0.7)
                case .waiting:
                    CubicKeyframe(-1.8, duration: 0.3)
                    CubicKeyframe(0, duration: 0.36)
                    LinearKeyframe(0, duration: 0.54)
                default:
                    LinearKeyframe(0, duration: 0.125)
                    MoveKeyframe(0.5)
                    LinearKeyframe(0.5, duration: 0.125)
                    MoveKeyframe(-0.5)
                    LinearKeyframe(-0.5, duration: 0.125)
                    MoveKeyframe(0.3)
                    LinearKeyframe(0.3, duration: 0.125)
                }
            }
        } else {
            art
        }
    }
}

extension EnvironmentValues {
    @Entry var faceMotion = true
}
