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
    /// Room for the widest island, its tallest list and its shadow. Only the island is drawn.
    static let canvas = CGSize(width: 760, height: 560)

    private let store: NotchStore
    private var panels: [CGDirectDisplayID: (panel: NotchPanel, island: IslandModel)] = [:]
    private var swipes: [CGDirectDisplayID: SwipeTracker] = [:]
    private var monitors: [Any] = []
    private var dragCount = 0
    private var settingsWatcher: SettingsWatcher?
    private var pointerScreen: CGDirectDisplayID?

    init(store: NotchStore) {
        self.store = store
        store.onPeek = { [weak self] peek in self?.peek(peek) }
        NotificationCenter.default.addObserver(
            forName: NSApplication.didChangeScreenParametersNotification, object: nil, queue: .main
        ) { [weak self] _ in self?.rebuild() }
        settingsWatcher = SettingsWatcher(path: store.options.settings) { [weak self] in
            self?.store.reloadSettings()
            self?.rebuild()
        }
        Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in self?.tick() }
        watchDrags()
        rebuild()
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
        if isAsk, settings.sound { NSSound(named: "Tink")?.play() }
        guard settings.peeks == .all || (isAsk && settings.peeks == .needsYou) else { return }
        if isAsk { Haptics.tick() }
        shown.forEach { $0.island.peek(peek, for: isAsk ? 6 : 3) }
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
        // An island whose screen went is moved to a screen that has none, so the draft in it is kept.
        var spare: [(panel: NotchPanel, island: IslandModel, swipe: SwipeTracker?)] = []
        for (id, entry) in panels where wanted[id] == nil {
            spare.append((entry.panel, entry.island, swipes[id]))
            panels[id] = nil
            swipes[id] = nil
        }
        for id in wanted.keys where panels[id] == nil && !spare.isEmpty {
            let moved = spare.removeLast()
            moved.island.set(.closed)
            panels[id] = (moved.panel, moved.island)
            swipes[id] = moved.swipe
        }
        spare.forEach { $0.panel.orderOut(nil) }
        for (id, screen) in wanted {
            let geometry = NotchGeometry.of(screen)
            let frame = CGRect(
                x: geometry.centerX - Self.canvas.width / 2,
                y: screen.frame.maxY - Self.canvas.height,
                width: Self.canvas.width,
                height: Self.canvas.height
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
            NotificationCenter.default.addObserver(
                forName: NSWindow.didBecomeKeyNotification, object: panel, queue: .main
            ) { [weak island] _ in island?.hasKeyboard = true }
            NotificationCenter.default.addObserver(
                forName: NSWindow.didResignKeyNotification, object: panel, queue: .main
            ) { [weak island] _ in island?.keyboardLost() }
            panel.orderFrontRegardless()
            panels[id] = (panel, island)
            let swipe = SwipeTracker()
            swipe.onPull = { [weak island] direction, travelled in island?.swipe(direction, travelled: travelled) }
            swipe.onSwipe = { [weak island] direction in island?.swiped(direction) }
            swipe.onEnd = { [weak island] in island?.swipeEnded() }
            swipes[id] = swipe
        }
        pointerScreen = wantedScreens().first.flatMap(Self.displayId)
        tick()
    }

    /// Hides the islands while Sikemux Dev's helper holds the notch, or over a full-screen app.
    private func tick() {
        if store.settings.displays == .pointer {
            let current = wantedScreens().first.flatMap(Self.displayId)
            if current != pointerScreen { rebuild() }
        }
        let yielding = !store.options.dev && store.settings.yieldToDev && Handover.devIsRunning(stateDir: store.options.stateDir)
        let needsYou = store.entries.contains { $0.mark == .needs } || !store.devices.isEmpty
        let fullScreen = FullScreen.displays()
        for (_, entry) in panels {
            entry.island.checkPointer()
        }
        for (id, entry) in panels {
            let covered: Bool
            switch store.settings.fullScreen {
            case .always: covered = false
            case .never: covered = fullScreen.contains(id)
            case .needsYou: covered = fullScreen.contains(id) && !needsYou
            }
            let hidden = yielding || covered
            if entry.island.hidden != hidden {
                withAnimation(.smooth(duration: 0.2)) { entry.island.hidden = hidden }
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
                  let id = self.panels.first(where: { $0.value.panel === window })?.key
            else { return event }
            self.swipes[id]?.handle(event)
            return event
        }
        if let swipes { monitors.append(swipes) }
    }

    private func drag(_ event: NSEvent) {
        let pasteboard = NSPasteboard(name: .drag)
        switch event.type {
        case .leftMouseDown:
            dragCount = pasteboard.changeCount
        case .leftMouseDragged:
            guard pasteboard.changeCount != dragCount, pasteboard.types?.contains(.fileURL) == true else { return }
            let mouse = NSEvent.mouseLocation
            for (_, entry) in panels {
                let island = entry.island
                guard let screen = entry.panel.screen, !island.hidden, island.mode != .drop else { continue }
                let frame = screen.frame
                let onScreen = mouse.x >= frame.minX && mouse.x < frame.maxX && mouse.y >= frame.minY && mouse.y <= frame.maxY
                let near = onScreen && mouse.y > frame.maxY - 140 && abs(mouse.x - island.geometry.centerX) < 420
                if near {
                    Haptics.tick()
                    island.set(.drop)
                }
            }
        case .leftMouseUp:
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
    static func displays() -> Set<CGDirectDisplayID> {
        guard let front = NSWorkspace.shared.frontmostApplication?.processIdentifier,
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
