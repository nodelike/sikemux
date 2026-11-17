import JavaScriptCore
import SwiftUI
#if canImport(AppKit)
import AppKit
#endif

enum BuddyState: String {
    case idle, working, waiting, done, error
}

/// Faces drawn by the app's own script, so the island's and the phone's are the ones the app shows.
@MainActor
final class BuddyArt {
    #if canImport(AppKit)
    typealias Art = NSImage
    #else
    typealias Art = SVGDrawing
    #endif

    static let shared = BuddyArt()

    private lazy var script: JSValue? = {
        let context = JSContext()
        context?.evaluateScript(buddiesScript)
        return context?.objectForKeyedSubscript("SikemuxBuddies")
    }()
    private var drawn: [String: Art] = [:]
    private var styled: [String: AgentFace] = [:]

    func art(_ face: AgentFace, state: BuddyState) -> Art? {
        let key = "\(face.pack) \(face.shape) \(face.color) \(face.box) \(state.rawValue)"
        if let art = drawn[key] { return art }
        let described: [String: Any] = ["pack": face.pack, "shape": face.shape, "color": face.color, "box": face.box]
        guard let svg = script?.objectForKeyedSubscript("notchFace")?.call(withArguments: [described, state.rawValue]),
              !svg.isNull, let markup = svg.toString(),
              let art = Self.read(markup)
        else { return nil }
        drawn[key] = art
        return art
    }

    /// The face a host's style gives an agent it has not given one of its own, such as a chat about to start.
    func styledFace(agentId: String, pack: String, shape: String) -> AgentFace? {
        let key = "\(agentId) \(pack) \(shape)"
        if let face = styled[key] { return face }
        guard let made = script?.objectForKeyedSubscript("styledFace")?.call(withArguments: [agentId, ["pack": pack, "shape": shape]]),
              !made.isNull,
              let pack = made.objectForKeyedSubscript("pack")?.toString(),
              let shape = made.objectForKeyedSubscript("shape")?.toString(),
              let color = made.objectForKeyedSubscript("color")?.toInt32(),
              let box = made.objectForKeyedSubscript("box")?.toString()
        else { return nil }
        let face = AgentFace(pack: pack, shape: shape, color: Int(color), box: box)
        styled[key] = face
        return face
    }

    private static func read(_ svg: String) -> Art? {
        #if canImport(AppKit)
        NSImage(data: Data(svg.utf8))
        #else
        SVGDrawing(svg)
        #endif
    }
}

/// A buddy with the app's bob, hop and shake, which move the whole buddy by units of its drawing.
struct BuddyFigure: View {
    let art: BuddyArt.Art
    let state: BuddyState
    let moves: Bool
    let size: CGFloat
    @Environment(\.stillMarks) private var still
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        if moves, !still, !reduceMotion, state == .working || state == .waiting || state == .error {
            let unit = unit
            let shakes = state == .error
            picture.keyframeAnimator(initialValue: CGFloat(0), repeating: true) { content, offset in
                shakes ? content.offset(x: offset * unit) : content.offset(y: offset * unit)
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
            picture
        }
    }

    #if canImport(AppKit)
    private var picture: some View {
        Image(nsImage: art).resizable().interpolation(.high).frame(width: size, height: size)
    }

    private var unit: CGFloat { size / max(art.size.width, 1) }
    #else
    private var picture: some View {
        SVGView(drawing: art).frame(width: size, height: size)
    }

    private var unit: CGFloat { size / max(art.viewBox.width, 1) }
    #endif
}

extension EnvironmentValues {
    @Entry var faceMotion = true
    /// Marks drawn as one still frame, for a picture made without a window, where Core Animation cannot run.
    @Entry public var stillMarks = false
}
