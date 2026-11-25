import AppKit
import SwiftUI

/// A borderless panel above the menu bar that never activates the app. While
/// the island is open it takes the keyboard when the prompt is clicked, and
/// gives it back without taking the person's app out of the front.
final class NotchPanel: NSPanel {
    var acceptsKey = false

    override var canBecomeKey: Bool { acceptsKey }
    override var canBecomeMain: Bool { false }

    init(frame: CGRect) {
        super.init(contentRect: frame, styleMask: [.borderless, .nonactivatingPanel, .utilityWindow, .hudWindow], backing: .buffered, defer: false)
        isFloatingPanel = true
        becomesKeyOnlyIfNeeded = true
        level = NSWindow.Level(rawValue: NSWindow.Level.mainMenu.rawValue + 3)
        collectionBehavior = [.canJoinAllSpaces, .stationary, .fullScreenAuxiliary, .ignoresCycle]
        isOpaque = false
        backgroundColor = .clear
        hasShadow = false
        isMovable = false
        hidesOnDeactivate = false
        isReleasedWhenClosed = false
        appearance = NSAppearance(named: .darkAqua)
    }
}

/// One island per screen the settings show it on, rebuilt whenever the screens change.
final class Panels {
    /// Room for the widest island and its shadow, as tall as the screen so a call's card in the list fits whole.
    /// Only the island is drawn.
    static let width: CGFloat = 760
    private static let maxParked = 2
    private static let fallbackInterval: TimeInterval = 8
    private static let dragCheckInterval: TimeInterval = 0.1

    private struct PanelEntry {
        let panel: NotchPanel
        let island: IslandModel
        var swipe: SwipeTracker?
        let observers: [NSObjectProtocol]
    }

    private let store: NotchStore
    private var panels: [CGDirectDisplayID: PanelEntry] = [:]
    /// Islands whose screen went, out of sight until a screen needs one. An external screen goes and
    /// comes back as the Mac wakes, when building a new island can stall for seconds.
    private var parked: [(id: CGDirectDisplayID, entry: PanelEntry)] = []
    private var monitors: [Any] = []
    private var workspaceObservers: [NSObjectProtocol] = []
    private var fallbackTimer: Timer?
    private var pointerTimer: Timer?
    private var dragCount = 0
    private var dragIsFile: Bool?
    private var lastDragCheck: TimeInterval = 0
    private var pointerScreen: CGDirectDisplayID?
    private var devRunning = false
    private var fullScreen: Set<CGDirectDisplayID> = []
    private var looking = false
    private var asleep = false

    init(store: NotchStore) {
        self.store = store
        store.onPeek = { [weak self] peek in self?.peek(peek) }
        store.onView = { [weak self] in self?.applyVisibility() }
        NotificationCenter.default.addObserver(
            forName: NSApplication.didChangeScreenParametersNotification, object: nil, queue: .main
        ) { [weak self] _ in self?.rebuild() }
        store.onSettings = { [weak self] in self?.rebuild() }
        watchWorkspace()
        watchDrags()
        rebuild()
    }

    private func watchWorkspace() {
        let center = NSWorkspace.shared.notificationCenter
        let refreshNames: [NSNotification.Name] = [
            NSWorkspace.didActivateApplicationNotification,
            NSWorkspace.activeSpaceDidChangeNotification,
            NSWorkspace.didLaunchApplicationNotification,
            NSWorkspace.didTerminateApplicationNotification,
        ]
        for name in refreshNames {
            workspaceObservers.append(
                center.addObserver(forName: name, object: nil, queue: .main) { [weak self] _ in
                    self?.refreshEnvironment()
                }
            )
        }
        workspaceObservers.append(
            center.addObserver(forName: NSWorkspace.willSleepNotification, object: nil, queue: .main) { [weak self] _ in
                guard let self else { return }
                self.asleep = true
                self.stopTimers()
                self.applyVisibility()
            }
        )
        workspaceObservers.append(
            center.addObserver(forName: NSWorkspace.didWakeNotification, object: nil, queue: .main) { [weak self] _ in
                guard let self else { return }
                self.asleep = false
                self.rescheduleTimers()
                self.tick()
            }
        )
    }

    private func stopTimers() {
        fallbackTimer?.invalidate()
        fallbackTimer = nil
        pointerTimer?.invalidate()
        pointerTimer = nil
    }

    private func rescheduleTimers() {
        stopTimers()
        guard !asleep else { return }
        if store.settings.displays == .pointer {
            pointerTimer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in
                self?.checkPointerScreen()
            }
        }
        let watchingDev = !store.options.dev && store.settings.yieldToDev
        let checksFullScreen = store.settings.fullScreen != .always
        if watchingDev || checksFullScreen {
            fallbackTimer = Timer.scheduledTimer(withTimeInterval: Self.fallbackInterval, repeats: true) { [weak self] _ in
                self?.refreshEnvironment()
            }
        }
    }

    private func checkPointerScreen() {
        guard !asleep, store.settings.displays == .pointer else { return }
        let current = wantedScreens().first.flatMap(Self.displayId)
        if current != pointerScreen { rebuild() }
    }

    /// Rings and peeks on every island that shows. A request first un-hides an
    /// island over a full-screen app, so its peek is not lost.
    private func peek(_ peek: NotchStore.Peek) {
        tick()
        let shown = panels.values.filter { !$0.island.hidden }
        guard !shown.isEmpty else { return }
        let settings = store.settings
        let isAsk: Bool
        if case .done = peek { isAsk = false } else { isAsk = true }
        if isAsk, settings.sound { Self.ring() }
        guard settings.peeks == .all || (isAsk && settings.peeks == .needsYou) else { return }
        if isAsk { Haptics.tick() }
        shown.forEach { $0.island.peek(peek, for: isAsk ? 6 : 3) }
    }

    private static let chime = NSSound(named: "Tink")
    private static let chimeQueue = DispatchQueue(label: "notch.chime", qos: .userInitiated)

    private static func ring() {
        chimeQueue.async { chime?.play() }
    }

    private static func displayId(_ screen: NSScreen) -> CGDirectDisplayID? {
        screen.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? CGDirectDisplayID
    }

    private func wantedScreens() -> [NSScreen] {
        let screens = NSScreen.screens
        switch store.settings.displays {
        case .all:
            return screens
        case .builtIn:
            let builtIn = screens.filter { Self.displayId($0).map { CGDisplayIsBuiltin($0) != 0 } ?? false }
            return builtIn.isEmpty ? Array(screens.prefix(1)) : builtIn
        case .pointer:
            let mouse = NSEvent.mouseLocation
            return screens.filter { $0.frame.contains(mouse) }.prefix(1).map { $0 }
        }
    }

    func rebuild() {
        let wanted = Dictionary(wantedScreens().compactMap { screen in Self.displayId(screen).map { ($0, screen) } },
                                uniquingKeysWith: { first, _ in first })
        let before = parked.count
        for (id, entry) in panels where wanted[id] == nil {
            entry.island.set(.closed)
            entry.island.hidden = true
            parked.append((id, entry))
            panels[id] = nil
        }
        var unparked: [NotchPanel] = []
        for id in wanted.keys where panels[id] == nil && !parked.isEmpty {
            // An island whose screen has just gone moves first, so a draft follows the screen that shows it.
            let own = parked.count > before ? nil : parked.firstIndex { $0.id == id }
            let moved = parked.remove(at: own ?? parked.count - 1)
            panels[id] = moved.entry
            unparked.append(moved.entry.panel)
        }
        while parked.count > Self.maxParked {
            destroy(parked.removeFirst().entry)
        }
        parked.forEach { $0.entry.panel.orderOut(nil) }
        for (id, screen) in wanted {
            let geometry = NotchGeometry.of(screen)
            let frame = CGRect(
                x: geometry.centerX - Self.width / 2,
                y: screen.frame.minY,
                width: Self.width,
                height: screen.frame.height
            )
            if let entry = panels[id] {
                entry.panel.setFrame(frame, display: true)
                entry.island.canvas = frame
                if entry.island.geometry != geometry { entry.island.geometry = geometry }
                continue
            }
            let island = IslandModel(geometry: geometry)
            island.canvas = frame
            let panel = NotchPanel(frame: frame)
            panel.contentView = NSHostingView(rootView: IslandView(store: store, island: island))
            island.keyboard = { [weak panel] keyboard in
                guard let panel else { return }
                switch keyboard {
                case .take:
                    panel.acceptsKey = true
                    panel.makeKey()
                case .give, .off:
                    // A panel that refuses the keyboard hands it straight back to the app in front.
                    // Ordering it out instead would cut its animation short and stop hover reaching it.
                    panel.acceptsKey = false
                    if panel.isKeyWindow { panel.resignKey() }
                    if keyboard == .give { DispatchQueue.main.async { panel.acceptsKey = true } }
                }
            }
            let becomeKey = NotificationCenter.default.addObserver(
                forName: NSWindow.didBecomeKeyNotification, object: panel, queue: .main
            ) { [weak island] _ in island?.hasKeyboard = true }
            let resignKey = NotificationCenter.default.addObserver(
                forName: NSWindow.didResignKeyNotification, object: panel, queue: .main
            ) { [weak island] _ in island?.keyboardLost() }
            panel.orderFrontRegardless()
            let swipe = SwipeTracker()
            swipe.onPull = { [weak island] direction, travelled in island?.swipe(direction, travelled: travelled) }
            swipe.onSwipe = { [weak island] direction in island?.swiped(direction) }
            swipe.onEnd = { [weak island] in island?.swipeEnded() }
            panels[id] = PanelEntry(panel: panel, island: island, swipe: swipe, observers: [becomeKey, resignKey])
        }
        unparked.filter { !$0.isVisible }.forEach { $0.orderFrontRegardless() }
        pointerScreen = wantedScreens().first.flatMap(Self.displayId)
        rescheduleTimers()
        tick()
    }

    private func destroy(_ entry: PanelEntry) {
        entry.island.set(.closed)
        entry.island.hidden = true
        entry.observers.forEach { NotificationCenter.default.removeObserver($0) }
        entry.panel.orderOut(nil)
        entry.panel.contentView = nil
        entry.panel.close()
    }

    /// Hides the islands while Sikemux Dev's helper holds the notch, or over a full-screen app.
    private func tick() {
        checkPointerScreen()
        refreshEnvironment()
        applyVisibility()
    }

    private func refreshEnvironment() {
        guard !asleep else { return }
        let watchingDev = !store.options.dev && store.settings.yieldToDev
        let checkFullScreen = store.settings.fullScreen != .always
        lookAround(watchingDev: watchingDev, checkFullScreen: checkFullScreen)
    }

    private func applyVisibility() {
        let watchingDev = !store.options.dev && store.settings.yieldToDev
        let yielding = watchingDev && devRunning
        let needsYou = store.entries.contains { $0.mark == .needs } || !store.devices.isEmpty
        for (id, entry) in panels {
            let covered: Bool
            switch store.settings.fullScreen {
            case .always: covered = false
            case .never: covered = fullScreen.contains(id)
            case .needsYou: covered = fullScreen.contains(id) && !needsYou
            }
            let hidden = asleep || yielding || covered
            if entry.island.hidden != hidden {
                withAnimation(.smooth(duration: 0.2)) { entry.island.hidden = hidden }
            }
        }
    }

    /// Opening Dev's lock file and listing the windows have each stalled the main thread for
    /// seconds on some Macs, so both run off it and update visibility once ready.
    private func lookAround(watchingDev: Bool, checkFullScreen: Bool) {
        if !watchingDev && !checkFullScreen {
            let changed = devRunning || !fullScreen.isEmpty
            devRunning = false
            fullScreen = []
            if changed { applyVisibility() }
            return
        }
        guard !looking else { return }
        looking = true
        let stateDir = store.options.stateDir
        let front = checkFullScreen ? NSWorkspace.shared.frontmostApplication?.processIdentifier : nil
        DispatchQueue.global(qos: .utility).async { [weak self] in
            let running = watchingDev && Handover.devIsRunning(stateDir: stateDir)
            let filled = checkFullScreen ? FullScreen.displays(filledBy: front) : []
            DispatchQueue.main.async {
                guard let self else { return }
                self.devRunning = running
                self.fullScreen = filled
                self.looking = false
                self.applyVisibility()
            }
        }
    }

    /// Opens the island's drop targets as a file is dragged towards the top of a screen.
    private func watchDrags() {
        let events: NSEvent.EventTypeMask = [.leftMouseDown, .leftMouseDragged, .leftMouseUp]
        let handler: (NSEvent) -> Void = { [weak self] event in self?.drag(event) }
        if let global = NSEvent.addGlobalMonitorForEvents(matching: events, handler: handler) {
            monitors.append(global)
        }
        let swipes = NSEvent.addLocalMonitorForEvents(matching: .scrollWheel) { [weak self] event in
            guard let self, let window = event.window,
                  let entry = self.panels.values.first(where: { $0.panel === window })
            else { return event }
            entry.swipe?.handle(event)
            return event
        }
        if let swipes { monitors.append(swipes) }
    }

    private func nearDropTargets(for mouse: NSPoint) -> [IslandModel] {
        panels.values.compactMap { entry in
            let island = entry.island
            guard let screen = entry.panel.screen, !island.hidden, island.mode != .drop else { return nil }
            let frame = screen.frame
            let onScreen = mouse.x >= frame.minX && mouse.x < frame.maxX && mouse.y >= frame.minY && mouse.y <= frame.maxY
            let near = onScreen && mouse.y > frame.maxY - 140 && abs(mouse.x - island.geometry.centerX) < 420
            return near ? island : nil
        }
    }

    private func drag(_ event: NSEvent) {
        switch event.type {
        case .leftMouseDown:
            dragCount = NSPasteboard(name: .drag).changeCount
            dragIsFile = nil
            lastDragCheck = 0
        case .leftMouseDragged:
            if dragIsFile == false { return }
            let targets = nearDropTargets(for: NSEvent.mouseLocation)
            guard !targets.isEmpty else { return }
            if dragIsFile == nil {
                guard event.timestamp - lastDragCheck >= Self.dragCheckInterval else { return }
                lastDragCheck = event.timestamp
                let pasteboard = NSPasteboard(name: .drag)
                guard pasteboard.changeCount != dragCount else { return }
                let hasFile = pasteboard.types?.contains(.fileURL) == true
                dragIsFile = hasFile
                guard hasFile else { return }
            }
            for island in targets {
                Haptics.tick()
                island.set(.drop)
            }
        case .leftMouseUp:
            dragIsFile = nil
            lastDragCheck = 0
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { [weak self] in
                self?.panels.values.forEach { entry in
                    if entry.island.mode == .drop {
                        entry.island.dropTarget = nil
                        entry.island.set(.closed)
                    }
                }
            }
        default:
            break
        }
    }
}

/// Which displays the frontmost app fills, as a full-screen app does.
enum FullScreen {
    static func displays(filledBy front: pid_t?) -> Set<CGDirectDisplayID> {
        guard let front,
              let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]]
        else { return [] }
        var filled: Set<CGDirectDisplayID> = []
        for window in windows {
            guard (window[kCGWindowOwnerPID as String] as? pid_t) == front,
                  (window[kCGWindowLayer as String] as? Int) == 0,
                  let bounds = window[kCGWindowBounds as String] as? [String: CGFloat],
                  let rect = CGRect(dictionaryRepresentation: bounds as CFDictionary)
            else { continue }
            var count: UInt32 = 0
            var ids = [CGDirectDisplayID](repeating: 0, count: 8)
            CGGetDisplaysWithRect(rect, 8, &ids, &count)
            for id in ids.prefix(Int(count)) where CGDisplayBounds(id) == rect {
                filled.insert(id)
            }
        }
        return filled
    }
}
