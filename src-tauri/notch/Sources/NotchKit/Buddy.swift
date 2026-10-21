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

/// Buddies drawn by the app's own script, so the island's are the ones the app shows.
@MainActor
final class BuddyArt {
    static let shared = BuddyArt()

    private lazy var draw: JSValue? = {
        let context = JSContext()
        context?.evaluateScript(buddiesScript)
        return context?.objectForKeyedSubscript("SikemuxBuddies")?.objectForKeyedSubscript("notchBuddy")
    }()
    private var drawn: PublishedBuddies?
    private var published: Any = NSNull()
    private var images: [String: NSImage] = [:]

    /// The agent's buddy, or nil when it shows its agent's mark instead.
    func image(_ buddies: PublishedBuddies, agentId: String, named: Bool, state: BuddyState) -> NSImage? {
        if buddies != drawn {
            drawn = buddies
            images = [:]
            published = (try? JSONEncoder().encode(buddies)).flatMap { try? JSONSerialization.jsonObject(with: $0) } ?? NSNull()
        }
        let key = "\(agentId) \(named) \(state.rawValue)"
        if let image = images[key] { return image }
        guard let drawn = draw?.call(withArguments: [published, agentId, named, state.rawValue]), !drawn.isNull,
              let svg = drawn.toString(),
              let image = NSImage(data: Data(svg.utf8))
        else { return nil }
        images[key] = image
        return image
    }
}

/// An agent's buddy while the app draws buddies or the person named it, otherwise its agent's mark.
struct AgentFace: View {
    let agentId: String?
    let provider: String
    var named = false
    var mark: Mark = .idle
    var size: CGFloat = 16
    @Environment(\.buddies) private var buddies

    var body: some View {
        if let buddies, let agentId,
           let image = BuddyArt.shared.image(buddies, agentId: agentId, named: named, state: BuddyState(mark)) {
            BuddyImage(image: image, state: BuddyState(mark), moves: buddies.motion, size: size)
        } else {
            AgentMark(provider: provider, size: size)
        }
    }
}

extension AgentFace {
    init(entry: Entry, size: CGFloat) {
        self.init(agentId: entry.item.agentId, provider: entry.provider, named: entry.item.name != nil, mark: entry.mark, size: size)
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
    @Entry var buddies: PublishedBuddies? = nil
}
