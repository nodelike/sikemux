import Foundation
import SwiftUI
#if canImport(AppKit)
import AppKit
import QuartzCore
#endif

enum BuddyState: String, CaseIterable {
    case idle, working, waiting, done, error

    var slotIndex: Int {
        switch self {
        case .idle: 0
        case .working: 1
        case .waiting: 2
        case .done: 3
        case .error: 4
        }
    }

    var animates: Bool {
        self == .working || self == .waiting || self == .error
    }
}

/// Faces drawn from the pre-generated buddy catalogue, so the island's and the phone's match the app without a JS engine.
@MainActor
final class BuddyArt {
    #if canImport(AppKit)
    typealias Art = NSImage
    #else
    typealias Art = SVGDrawing
    #endif

    static let shared = BuddyArt()

    private static let packsById: [String: BuddyPackData] = Dictionary(
        uniqueKeysWithValues: buddyPacks.map { ($0.id, $0) }
    )
    private static let fullBox = "0 0 24 24"

    private var drawn: [String: Art] = [:]
    private var styled: [String: AgentFace] = [:]

    func art(_ face: AgentFace, state: BuddyState) -> Art? {
        let key = "\(face.pack) \(face.shape) \(face.color) \(face.box) \(state.rawValue)"
        if let art = drawn[key] { return art }
        guard let markup = Self.svg(face, state: state),
              let art = Self.read(markup)
        else { return nil }
        drawn[key] = art
        return art
    }

    /// The face a host's style gives an agent it has not given one of its own, such as a chat about to start.
    func styledFace(agentId: String, pack: String, shape: String) -> AgentFace? {
        let key = "\(agentId) \(pack) \(shape)"
        if let face = styled[key] { return face }
        guard let face = Self.resolveFace(agentId: agentId, pack: pack, shape: shape) else { return nil }
        styled[key] = face
        return face
    }

    static func svg(_ face: AgentFace, state: BuddyState) -> String? {
        guard let pack = packsById[face.pack],
              let shapeIndex = pack.shapes.firstIndex(of: face.shape),
              pack.colors > 0
        else { return nil }
        let colorIndex = ((face.color % pack.colors) + pack.colors) % pack.colors
        let entryIndex = (shapeIndex * pack.colors + colorIndex) * BuddyState.allCases.count + state.slotIndex
        guard let encoded = Data(base64Encoded: pack.compressed),
              let inflated = try? (encoded as NSData).decompressed(using: .zlib) as Data,
              let inner = nthSegment(in: inflated, index: entryIndex)
        else { return nil }
        let sparkle: String
        if state == .done {
            let parts = face.box.split(separator: " ").compactMap { Double($0) }
            if parts.count >= 3 {
                let x = parts[0] + parts[2] - parts[2] * 0.08
                let y = parts[1] + parts[2] * 0.1
                sparkle = "<path class=\"bd-twinkle\" d=\"m\(x) \(y - 1.4) .45 1 1 .4-1 .45-.45 1-.45-1-1-.45 1-.4Z\" fill=\"#ffe27a\"/>"
            } else {
                sparkle = ""
            }
        } else {
            sparkle = ""
        }
        return "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"\(face.box)\" aria-hidden=\"true\"><g>\(inner)\(sparkle)</g></svg>"
    }

    static func resolveFace(agentId: String, pack packId: String, shape shapeId: String) -> AgentFace? {
        guard let firstPack = buddyPacks.first else { return nil }
        let h = hash(agentId)
        let everyPack = packId == "all"
        let chosenPack = everyPack
            ? buddyPacks[Int(h % UInt32(buddyPacks.count))]
            : (packsById[packId] ?? firstPack)
        guard !chosenPack.shapes.isEmpty, chosenPack.colors > 0 else { return nil }
        let chosenShape: String
        if everyPack || shapeId == "mixed" {
            chosenShape = chosenPack.shapes[Int((h >> 8) % UInt32(chosenPack.shapes.count))]
        } else {
            chosenShape = chosenPack.shapes.contains(shapeId) ? shapeId : chosenPack.shapes[0]
        }
        let colorIndex = Int(h % UInt32(chosenPack.colors))
        return AgentFace(pack: chosenPack.id, shape: chosenShape, color: colorIndex, box: fullBox)
    }

    static func hash(_ text: String) -> UInt32 {
        var h: UInt32 = 2_166_136_261
        for unit in text.utf16 {
            h ^= UInt32(unit)
            h = h &* 16_777_619
        }
        h ^= h >> 16
        h = h &* 0x85eb_ca6b
        h ^= h >> 13
        return h
    }

    private static func nthSegment(in data: Data, index target: Int) -> String? {
        data.withUnsafeBytes { raw in
            let bytes = raw.bindMemory(to: UInt8.self)
            var current = 0
            var start = 0
            for i in 0..<bytes.count where bytes[i] == 0 {
                if current == target {
                    return String(decoding: bytes[start..<i], as: UTF8.self)
                }
                current += 1
                start = i + 1
            }
            guard current == target else { return nil }
            return String(decoding: bytes[start..<bytes.count], as: UTF8.self)
        }
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
        #if canImport(AppKit)
        if moves, !still, !reduceMotion, state.animates {
            AnimatedBuddy(art: art, state: state, size: size)
                .frame(width: size, height: size)
        } else {
            picture
        }
        #else
        if moves, !still, !reduceMotion, state.animates {
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
        #endif
    }

    #if canImport(AppKit)
    private var picture: some View {
        Image(nsImage: art).resizable().interpolation(.high).frame(width: size, height: size)
    }
    #else
    private var picture: some View {
        SVGView(drawing: art).frame(width: size, height: size)
    }

    private var unit: CGFloat { size / max(art.viewBox.width, 1) }
    #endif
}

#if canImport(AppKit)
private struct AnimatedBuddy: NSViewRepresentable {
    let art: NSImage
    let state: BuddyState
    let size: CGFloat

    func makeNSView(context: Context) -> BuddyFigureView {
        BuddyFigureView(art: art, state: state, size: size)
    }

    func updateNSView(_ view: BuddyFigureView, context: Context) {
        view.configure(art: art, state: state, size: size)
    }

    func sizeThatFits(_ proposal: ProposedViewSize, nsView: BuddyFigureView, context: Context) -> CGSize? {
        CGSize(width: size, height: size)
    }
}

/// Core Animation runs the bob, hop and shake on the GPU so SwiftUI does no work per frame.
final class BuddyFigureView: NSView {
    private static let animationKey = "buddyMotion"

    private struct MotionSpec: Equatable {
        let state: BuddyState
        let unit: CGFloat
        let flipped: Bool
    }

    private struct RenderKey: Hashable {
        let art: ObjectIdentifier
        let size: CGFloat
        let scale: CGFloat
    }

    private static var imageCache: [RenderKey: CGImage] = [:]

    private let imageLayer = CALayer()
    private var art: NSImage
    private var state: BuddyState
    private var figureSize: CGFloat
    private var renderedKey: RenderKey?
    private var activeMotion: MotionSpec?
    private var occlusionObserver: NSObjectProtocol?

    init(art: NSImage, state: BuddyState, size: CGFloat) {
        self.art = art
        self.state = state
        self.figureSize = size
        super.init(frame: CGRect(x: 0, y: 0, width: size, height: size))
        wantsLayer = true
        layerContentsRedrawPolicy = .never
        layer?.masksToBounds = false
        imageLayer.contentsGravity = .resize
        imageLayer.frame = bounds
        layer?.addSublayer(imageLayer)
        updateContents()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    deinit {
        if let observer = occlusionObserver {
            NotificationCenter.default.removeObserver(observer)
        }
    }

    override var intrinsicContentSize: NSSize { NSSize(width: figureSize, height: figureSize) }

    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    func configure(art: NSImage, state: BuddyState, size: CGFloat) {
        let sizeChanged = figureSize != size
        self.art = art
        self.state = state
        figureSize = size
        if sizeChanged {
            invalidateIntrinsicContentSize()
            needsLayout = true
        }
        updateContents()
        updateAnimation()
    }

    override func layout() {
        super.layout()
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        imageLayer.frame = bounds
        CATransaction.commit()
    }

    override func viewDidChangeBackingProperties() {
        super.viewDidChangeBackingProperties()
        updateContents()
    }

    override func viewDidHide() {
        super.viewDidHide()
        updateAnimation()
    }

    override func viewDidUnhide() {
        super.viewDidUnhide()
        updateAnimation()
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        if let observer = occlusionObserver {
            NotificationCenter.default.removeObserver(observer)
            occlusionObserver = nil
        }
        if let window {
            occlusionObserver = NotificationCenter.default.addObserver(
                forName: NSWindow.didChangeOcclusionStateNotification,
                object: window,
                queue: .main
            ) { [weak self] _ in
                MainActor.assumeIsolated {
                    self?.updateAnimation()
                }
            }
            updateContents()
        }
        updateAnimation()
    }

    private var shouldAnimate: Bool {
        guard state.animates, let window, !isHiddenOrHasHiddenAncestor else { return false }
        return window.occlusionState.contains(.visible)
    }

    private func updateContents() {
        let scale = window?.backingScaleFactor ?? NSScreen.main?.backingScaleFactor ?? 2
        let key = RenderKey(art: ObjectIdentifier(art), size: figureSize, scale: scale)
        guard renderedKey != key else { return }
        renderedKey = key
        let image = Self.imageCache[key] ?? {
            let rendered = Self.rasterize(art, size: figureSize, scale: scale)
            if let rendered { Self.imageCache[key] = rendered }
            return rendered
        }()
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        imageLayer.contentsScale = scale
        imageLayer.contents = image
        CATransaction.commit()
    }

    private func updateAnimation() {
        guard shouldAnimate else {
            if activeMotion != nil {
                imageLayer.removeAnimation(forKey: Self.animationKey)
                activeMotion = nil
            }
            return
        }
        let unit = figureSize / max(art.size.width, 1)
        let flipped = layer?.isGeometryFlipped ?? false
        let wanted = MotionSpec(state: state, unit: unit, flipped: flipped)
        guard activeMotion != wanted else { return }
        activeMotion = wanted
        imageLayer.add(Self.makeAnimation(state: state, unit: unit, flipped: flipped), forKey: Self.animationKey)
    }

    static func makeAnimation(state: BuddyState, unit: CGFloat, flipped: Bool = false) -> CAKeyframeAnimation {
        let up: CGFloat = flipped ? -1 : 1
        switch state {
        case .working:
            let anim = CAKeyframeAnimation(keyPath: "transform.translation.y")
            anim.values = [0, 0.8 * unit * up, 0]
            anim.keyTimes = [0, 0.5, 1]
            anim.timingFunctions = [
                CAMediaTimingFunction(name: .easeInEaseOut),
                CAMediaTimingFunction(name: .easeInEaseOut),
            ]
            anim.duration = 1.4
            anim.repeatCount = .infinity
            anim.isRemovedOnCompletion = false
            return anim
        case .waiting:
            let hop = CAMediaTimingFunction(controlPoints: 0.3, 0, 0.3, 1)
            let anim = CAKeyframeAnimation(keyPath: "transform.translation.y")
            anim.values = [0, 1.8 * unit * up, 0, 0]
            anim.keyTimes = [0, 0.25, 0.55, 1]
            anim.timingFunctions = [hop, hop, CAMediaTimingFunction(name: .linear)]
            anim.duration = 1.2
            anim.repeatCount = .infinity
            anim.isRemovedOnCompletion = false
            return anim
        default:
            let anim = CAKeyframeAnimation(keyPath: "transform.translation.x")
            anim.values = [0, 0.5 * unit, -0.5 * unit, 0.3 * unit]
            anim.keyTimes = [0, 0.25, 0.5, 0.75, 1]
            anim.calculationMode = .discrete
            anim.duration = 0.5
            anim.repeatCount = .infinity
            anim.isRemovedOnCompletion = false
            return anim
        }
    }

    private static func rasterize(_ image: NSImage, size: CGFloat, scale: CGFloat) -> CGImage? {
        let pixels = max(1, Int((size * scale).rounded(.up)))
        guard let context = CGContext(
            data: nil,
            width: pixels,
            height: pixels,
            bitsPerComponent: 8,
            bytesPerRow: 0,
            space: CGColorSpaceCreateDeviceRGB(),
            bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue
        ) else { return nil }
        context.scaleBy(x: scale, y: scale)
        let graphics = NSGraphicsContext(cgContext: context, flipped: false)
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = graphics
        graphics.imageInterpolation = .high
        image.draw(in: NSRect(origin: .zero, size: NSSize(width: size, height: size)), from: .zero, operation: .copy, fraction: 1)
        NSGraphicsContext.restoreGraphicsState()
        return context.makeImage()
    }
}
#endif

extension EnvironmentValues {
    @Entry var faceMotion = true
    /// Marks drawn as one still frame, for a picture made without a window, where Core Animation cannot run.
    @Entry public var stillMarks = false
}
