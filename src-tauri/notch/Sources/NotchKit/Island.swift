import AppKit
import Observation
import SwiftUI

/// Where a screen's notch is, or where the island sits on a screen without one.
struct NotchGeometry: Equatable {
    /// Zero on a screen without a notch: the island is one pill there.
    let notchWidth: CGFloat
    let height: CGFloat
    /// The middle of the notch, in screen coordinates.
    let centerX: CGFloat

    var hasNotch: Bool { notchWidth > 0 }

    static func of(_ screen: NSScreen) -> NotchGeometry {
        let frame = screen.frame
        let menuBar = frame.maxY - screen.visibleFrame.maxY
        if screen.safeAreaInsets.top > 0, let left = screen.auxiliaryTopLeftArea, let right = screen.auxiliaryTopRightArea {
            let width = frame.width - left.width - right.width
            return NotchGeometry(notchWidth: width, height: screen.safeAreaInsets.top, centerX: frame.minX + left.width + width / 2)
        }
        return NotchGeometry(notchWidth: 0, height: menuBar > 0 ? menuBar : 24, centerX: frame.midX)
    }
}

enum Motion {
    static let open = Animation.spring(response: 0.42, dampingFraction: 0.8)
    static let close = Animation.spring(response: 0.45, dampingFraction: 1.0)
    static let hover = Animation.interactiveSpring(response: 0.38, dampingFraction: 0.8)
    static let content = Animation.smooth(duration: 0.35)
}

/// The island on one screen: whether it is open, and what is being typed in it.
@Observable
final class IslandModel {
    enum Mode: Equatable {
        case closed
        case peekAsk(String)
        case peekDone(String)
        /// A device asking to connect: open until it is answered, whatever the pointer does.
        case connect(String)
        case open
        case drop
    }

    enum Tab { case inbox, compose }
    enum Menu { case model, effort, project }

    var mode: Mode = .closed
    var hovering = false
    var tab: Tab = .inbox
    var menu: Menu?
    var draft = ""
    var provider = "claude"
    var model: String?
    var effort: String?
    var project: Project?
    var yolo = true
    var attachments: [String] = []
    var dropTarget: String?
    /// Whether this island's panel holds the keyboard, as it does once the prompt is clicked.
    var hasKeyboard = false
    /// The file chooser is up: the island stays open behind it.
    var choosingFiles = false
    /// Hidden while Sikemux Dev's helper holds the notch, or over a full-screen app.
    var hidden = false
    var geometry: NotchGeometry
    /// How far a two-finger swipe has pulled the island: positive down, negative up.
    var pull: CGFloat = 0
    /// Whether the agent list is taller than the island shows, so it scrolls instead of closing.
    var listScrolls = false

    /// The island leans into a swipe: about a percent per twentieth of the way, never under 60%.
    var pullScale: CGFloat { pull == 0 ? 1 : max(0.6, 1 + pull * 0.01) }

    /// Whether the island holds the keyboard: `take` it now, `give` it back but
    /// take it when the prompt is clicked, or `off` while closed.
    enum Keyboard { case take, give, off }

    @ObservationIgnored var keyboard: ((Keyboard) -> Void)?
    /// The panel this island is drawn in, and the drawn shape within it, so the
    /// pointer can be checked against the shape itself.
    @ObservationIgnored var canvas: CGRect = .zero
    @ObservationIgnored var shapeFrame: CGRect = .zero
    @ObservationIgnored private var peekTimer: DispatchWorkItem?
    @ObservationIgnored private var hoverTimer: DispatchWorkItem?

    init(geometry: NotchGeometry) {
        self.geometry = geometry
    }

    var isOpen: Bool { mode == .open || mode == .drop }

    func set(_ mode: Mode, animation: Animation? = nil) {
        Debug.log("set \(mode)")
        withAnimation(animation ?? (mode == .closed ? Motion.close : Motion.open)) {
            self.mode = mode
            if mode != .open { menu = nil }
        }
        if mode == .open { keyboard?(.give) } else if mode == .closed { keyboard?(.off) }
    }

    /// Typing in the prompt keeps the island open when the pointer leaves; an empty prompt does not.
    var holdsOpen: Bool {
        if choosingFiles { return true }
        let started = !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || !attachments.isEmpty
        return hasKeyboard && tab == .compose && started
    }

    /// Whether the pointer is over the drawn island, give or take a few points.
    var pointerInside: Bool {
        guard !hidden, shapeFrame.width > 0 else { return false }
        let shape = CGRect(
            x: canvas.minX + shapeFrame.minX,
            y: canvas.maxY - shapeFrame.maxY,
            width: shapeFrame.width,
            height: shapeFrame.height
        )
        return shape.insetBy(dx: -4, dy: -4).contains(NSEvent.mouseLocation)
    }

    /// The person clicked somewhere else: the island closes, and the draft waits for next time.
    func keyboardLost() {
        hasKeyboard = false
        guard isOpen, !choosingFiles, !pointerInside else { return }
        set(.closed)
    }

    /// Shows a peek unless the island is open, hidden, or already asking for something more pressing.
    func peek(_ peek: NotchStore.Peek, for seconds: Double) {
        if case .connect(let id) = peek { return askToConnect(id) }
        guard !isOpen, !hidden, !asksToConnect else { return }
        switch peek {
        case .ask(let id):
            set(.peekAsk(id))
        case .done(let id):
            if case .peekAsk = mode { return }
            set(.peekDone(id))
        case .connect:
            return
        }
        peekTimer?.cancel()
        let timer = DispatchWorkItem { [weak self] in
            guard let self, !self.hovering, !self.isOpen else { return }
            self.set(.closed)
        }
        peekTimer = timer
        DispatchQueue.main.asyncAfter(deadline: .now() + seconds, execute: timer)
    }

    /// Opens on the oldest device waiting, unless the person is busy in the island; later ones wait their turn.
    private func askToConnect(_ id: String) {
        guard !hidden, !holdsOpen, mode != .drop, !asksToConnect else { return }
        peekTimer?.cancel()
        set(.connect(id))
    }

    var asksToConnect: Bool {
        if case .connect = mode { return true }
        return false
    }

    /// Once a device is answered here or in Sikemux, or its request lapses, the next one asks or the island folds back.
    func devicesChanged(_ devices: [PendingDevice]) {
        guard case .connect(let id) = mode, !devices.contains(where: { $0.id == id }) else { return }
        if let next = devices.first {
            set(.connect(next.id))
        } else {
            set(hovering ? .open : .closed)
        }
    }

    func pointer(entered: Bool, opensOnHover: Bool) {
        Debug.log("pointer entered \(entered) hovering \(hovering) mode \(mode)")
        hoverTimer?.cancel()
        if entered {
            if !hovering, mode == .closed { Haptics.tick() }
            withAnimation(Motion.hover) { hovering = true }
            // A permission peek keeps its size under the pointer, so its buttons stay where they are.
            guard opensOnHover, mode == .closed || isPeekDone else { return }
            schedule(after: 0.3) { $0.set(.open) }
        } else {
            schedule(after: 0.1) { island in
                // Reordering the panel to hand back the keyboard reports a leave the pointer never made.
                guard !island.pointerInside else { return }
                island.left()
            }
        }
    }

    /// Catches a leave macOS never reported, as when the island shrinks or hides under a still pointer.
    func checkPointer() {
        guard !pointerInside else { return }
        if hovering || (mode == .open && !holdsOpen) {
            Debug.log("pointer left unreported, mode \(mode)")
            hoverTimer?.cancel()
            left()
        }
    }

    private func left() {
        withAnimation(Motion.hover) { hovering = false }
        switch mode {
        case .open where !holdsOpen, .peekAsk, .peekDone:
            set(.closed)
        default:
            break
        }
    }

    /// A peek ends once what it shows leaves the inbox: the request was answered, here or anywhere else, or the agent closed.
    func entriesChanged(_ entries: [Entry]) {
        switch mode {
        case .peekAsk(let id) where !entries.contains { $0.id == id }:
            set(hovering ? .open : .closed)
        case .peekDone(let id) where !entries.contains { $0.id == id }:
            set(.closed)
        default:
            break
        }
    }

    private var isPeekDone: Bool {
        if case .peekDone = mode { return true }
        return false
    }

    /// A two-finger swipe over the island: down opens it, up closes it, each
    /// with a tick once the fingers have gone far enough.
    func swipe(_ direction: SwipeTracker.Direction, travelled: CGFloat) {
        switch (direction, mode) {
        case (.down, .closed), (.down, .peekDone):
            pull = travelled / SwipeTracker.threshold * 20
        case (.up, .peekAsk), (.up, .peekDone), (.up, .connect):
            pull = -travelled / SwipeTracker.threshold * 20
        case (.up, .open) where !holdsOpen && menu == nil && (tab == .compose || !listScrolls):
            pull = -travelled / SwipeTracker.threshold * 20
        default:
            pull = 0
        }
    }

    func swiped(_ direction: SwipeTracker.Direction) {
        guard pull != 0 else { return }
        pull = 0
        Haptics.tick()
        set(direction == .down ? .open : .closed)
    }

    func swipeEnded() {
        pull = 0
    }

    private func schedule(after seconds: Double, _ work: @escaping (IslandModel) -> Void) {
        let timer = DispatchWorkItem { [weak self] in
            if let self { work(self) }
        }
        hoverTimer = timer
        DispatchQueue.main.asyncAfter(deadline: .now() + seconds, execute: timer)
    }
}

/// The notch's outline: the top corners curve outward into the menu bar and
/// the bottom ones inward. Both radii animate with the size.
struct NotchShape: Shape {
    var top: CGFloat
    var bottom: CGFloat

    var animatableData: AnimatablePair<CGFloat, CGFloat> {
        get { AnimatablePair(top, bottom) }
        set {
            top = newValue.first
            bottom = newValue.second
        }
    }

    func path(in rect: CGRect) -> Path {
        let t = min(top, rect.width / 4, rect.height / 2)
        let b = min(bottom, rect.width / 4, max(0, rect.height - t))
        var path = Path()
        path.move(to: CGPoint(x: rect.minX, y: rect.minY))
        path.addQuadCurve(to: CGPoint(x: rect.minX + t, y: rect.minY + t), control: CGPoint(x: rect.minX + t, y: rect.minY))
        path.addLine(to: CGPoint(x: rect.minX + t, y: rect.maxY - b))
        path.addQuadCurve(to: CGPoint(x: rect.minX + t + b, y: rect.maxY), control: CGPoint(x: rect.minX + t, y: rect.maxY))
        path.addLine(to: CGPoint(x: rect.maxX - t - b, y: rect.maxY))
        path.addQuadCurve(to: CGPoint(x: rect.maxX - t, y: rect.maxY - b), control: CGPoint(x: rect.maxX - t, y: rect.maxY))
        path.addLine(to: CGPoint(x: rect.maxX - t, y: rect.minY + t))
        path.addQuadCurve(to: CGPoint(x: rect.maxX, y: rect.minY), control: CGPoint(x: rect.maxX - t, y: rect.minY))
        path.closeSubpath()
        return path
    }
}

enum Radii {
    static let closed = (top: CGFloat(6), bottom: CGFloat(14))
    static let peek = (top: CGFloat(14), bottom: CGFloat(22))
    static let open = (top: CGFloat(19), bottom: CGFloat(24))
}
