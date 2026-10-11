import AppKit
@testable import NotchKit
import SwiftUI

// Renders each state of the island against demo agents, to compare with
// src-tauri/notch/design/screens.html: `swift run notch-snapshots <dir>`.

let fixture = #"""
{
  "workspace": {
    "projects": [
      {"id": "p-sikemux", "name": "sikemux", "path": "/Users/me/sikemux"},
      {"id": "p-server", "name": "server", "path": "/Users/me/server"},
      {"id": "p-front", "name": "sikemux-front", "path": "/Users/me/sikemux-front"}
    ],
    "launchers": [
      {"id": "claude", "provider": "claude", "label": "Claude", "permissionMode": "bypass", "configOptions": [
        {"type": "select", "id": "model", "name": "Model", "currentValue": "opus", "options": [
          {"value": "opus", "name": "Opus", "description": "Opus 5.5 with 1M context"},
          {"value": "sonnet", "name": "Sonnet", "description": "Sonnet 5.5"},
          {"value": "haiku", "name": "Haiku", "description": "Haiku 4.5"}
        ]},
        {"type": "select", "id": "effort", "name": "Effort", "category": "thought_level", "currentValue": "high", "options": [
          {"value": "low", "name": "Low"}, {"value": "medium", "name": "Medium"}, {"value": "high", "name": "High"}, {"value": "max", "name": "Max"}
        ]}
      ]},
      {"id": "codex", "provider": "codex", "label": "Codex", "permissionMode": "bypass", "configOptions": null},
      {"id": "opencode", "provider": "opencode", "label": "OpenCode", "permissionMode": "bypass", "configOptions": null},
      {"id": "grok", "provider": "grok", "label": "Grok", "permissionMode": "bypass", "configOptions": null}
    ],
    "palette": {},
    "backdrop": {"texture": false, "image": null}
  },
  "sessions": [],
  "chats": [
    {"agentId": "a1", "provider": "claude", "title": "Fix the login flake", "cwd": "/Users/me/sikemux", "sessionId": "s1", "state": "ready",
     "running": true, "pendingPermissions": ["p1"], "subagents": 0, "tasks": 0, "permissionMode": "default", "asleep": false, "unread": true},
    {"agentId": "a2", "provider": "codex", "title": "Refactor the auth middleware", "cwd": "/Users/me/server", "sessionId": "s2", "state": "ready",
     "running": true, "pendingPermissions": [], "subagents": 3, "tasks": 0, "permissionMode": "bypass", "asleep": false, "unread": false},
    {"agentId": "a3", "provider": "opencode", "title": "Draft the notch spec", "cwd": "/Users/me/sikemux", "sessionId": "s3", "state": "ready",
     "running": true, "pendingPermissions": [], "subagents": 0, "tasks": 0, "permissionMode": "bypass", "asleep": false, "unread": false},
    {"agentId": "a5", "provider": "codex", "title": "Read the release notes", "cwd": "/Users/me/sikemux", "sessionId": "s5", "state": "ready",
     "running": false, "pendingPermissions": [], "subagents": 0, "tasks": 2, "permissionMode": "bypass", "asleep": false, "unread": false},
    {"agentId": "a6", "provider": "claude", "title": "Sikemux bug", "cwd": "/Users/me/sikemux", "sessionId": null, "state": "stopped",
     "running": false, "pendingPermissions": [], "subagents": 0, "tasks": 0, "permissionMode": "bypass", "asleep": true, "unread": false},
    {"agentId": "a7", "provider": "claude", "title": "Tidy the changelog", "cwd": "/Users/me/sikemux", "sessionId": "s7", "state": "ready",
     "running": false, "pendingPermissions": [], "subagents": 0, "tasks": 0, "permissionMode": "bypass", "asleep": false, "unread": false},
    {"agentId": "a4", "provider": "grok", "title": "Bump Astro to 6", "cwd": "/Users/me/sikemux-front", "sessionId": "s4", "state": "ready",
     "running": false, "pendingPermissions": [], "subagents": 0, "tasks": 0, "permissionMode": "bypass", "asleep": false, "unread": true}
  ],
  "attentions": [
    {"id": "p1", "kind": "permission", "agentId": "a1", "provider": "claude", "cwd": "/Users/me/sikemux", "at": NOW,
     "request": {"toolCall": {"title": "Run tests", "rawInput": {"command": "pnpm test --filter core -- --runInBand"}},
                 "options": [{"optionId": "o1", "kind": "allow_once"}, {"optionId": "o2", "kind": "allow_always"}, {"optionId": "o3", "kind": "reject_once"}]}}
  ]
}
"""#

let arguments = CommandLine.arguments
let outputDir = URL(fileURLWithPath: arguments.count > 1 ? arguments[1] : "/tmp/notch-snapshots")
try? FileManager.default.createDirectory(at: outputDir, withIntermediateDirectories: true)

let fontsDir = URL(fileURLWithPath: #filePath).deletingLastPathComponent().appendingPathComponent("../../Fonts").standardized.path
Theme.registerFonts(in: fontsDir)
_ = NSApplication.shared
NSApp.setActivationPolicy(.prohibited)

let now = String(Int(Date().timeIntervalSince1970 * 1000) - 134_000)
let full = try! JSONDecoder().decode(DeviceView.self, from: Data(fixture.replacingOccurrences(of: "NOW", with: now).utf8))

func view(keeping ids: Set<String>?) -> DeviceView {
    guard let ids else { return full }
    return DeviceView(
        workspace: full.workspace,
        sessions: full.sessions,
        chats: full.chats.filter { ids.contains($0.agentId) },
        attentions: full.attentions.filter { ids.contains($0.agentId) }
    )
}

let notch = NotchGeometry(notchWidth: 221, height: 38, centerX: 0)
let plain = NotchGeometry(notchWidth: 0, height: 24, centerX: 0)

func render(_ name: String, agents: Set<String>? = nil, geometry: NotchGeometry = notch, height: CGFloat = 140, dev: Bool = false,
            devices: [PendingDevice] = [], setup: (IslandModel) -> Void = { _ in })
{
    var options = Options()
    options.dev = dev
    let store = NotchStore(options: options)
    store.apply(view(keeping: agents))
    store.apply(devices, now: Date().addingTimeInterval(-8))
    let island = IslandModel(geometry: geometry)
    setup(island)
    let size = CGSize(width: 760, height: height)
    let root = ZStack(alignment: .top) {
        LinearGradient(colors: [Color(hex: "#3b2a6e"), Color(hex: "#141024"), Color(hex: "#1f3b6b")], startPoint: .bottomLeading, endPoint: .topTrailing)
        Color.black.frame(height: 10).frame(maxHeight: .infinity, alignment: .top)
        IslandView(store: store, island: island).padding(.top, 10)
    }
    .frame(width: size.width, height: size.height)
    let window = NSWindow(contentRect: CGRect(origin: .zero, size: size), styleMask: .borderless, backing: .buffered, defer: false)
    let host = NSHostingView(rootView: root)
    host.frame = CGRect(origin: .zero, size: size)
    window.contentView = host
    host.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.8))
    guard let bitmap = host.bitmapImageRepForCachingDisplay(in: host.bounds) else { return }
    host.cacheDisplay(in: host.bounds, to: bitmap)
    let url = outputDir.appendingPathComponent("\(name).png")
    try? bitmap.representation(using: .png, properties: [:])?.write(to: url)
    print(url.path)
}

/// The island opening, frame by frame: closed, then every 50 ms after it is told to open.
func film(_ name: String, height: CGFloat = 440, frames: Int = 12, _ change: @escaping (IslandModel) -> Void) {
    let store = NotchStore(options: Options())
    store.apply(full)
    let island = IslandModel(geometry: notch)
    let size = CGSize(width: 760, height: height)
    let root = ZStack(alignment: .top) {
        Color(hex: "#2a2140")
        IslandView(store: store, island: island)
    }
    .frame(width: size.width, height: size.height)
    let window = NSWindow(contentRect: CGRect(origin: .zero, size: size), styleMask: .borderless, backing: .buffered, defer: false)
    let host = NSHostingView(rootView: root)
    host.frame = CGRect(origin: .zero, size: size)
    window.contentView = host
    window.orderFrontRegardless()
    RunLoop.main.run(until: Date().addingTimeInterval(0.5))
    var shots: [NSBitmapImageRep] = []
    func capture() {
        host.layoutSubtreeIfNeeded()
        if let bitmap = host.bitmapImageRepForCachingDisplay(in: host.bounds) {
            host.cacheDisplay(in: host.bounds, to: bitmap)
            shots.append(bitmap)
        }
    }
    capture()
    change(island)
    for _ in 0..<frames {
        RunLoop.main.run(until: Date().addingTimeInterval(0.05))
        capture()
    }
    for (index, shot) in shots.enumerated() {
        let url = outputDir.appendingPathComponent("\(name)-\(String(format: "%02d", index)).png")
        try? shot.representation(using: .png, properties: [:])?.write(to: url)
    }
    let strip = NSImage(size: NSSize(width: size.width, height: size.height * CGFloat(shots.count)))
    strip.lockFocus()
    for (index, shot) in shots.enumerated() {
        shot.draw(in: NSRect(x: 0, y: size.height * CGFloat(shots.count - 1 - index), width: size.width, height: size.height))
    }
    strip.unlockFocus()
    if let tiff = strip.tiffRepresentation, let bitmap = NSBitmapImageRep(data: tiff) {
        let url = outputDir.appendingPathComponent("\(name).png")
        try? bitmap.representation(using: .png, properties: [:])?.write(to: url)
        print(url.path)
    }
}

if arguments.contains("--film") {
    film("film-open") { $0.set(.open) }
    exit(0)
}

render("C1-idle", agents: [])
render("C2b-only-idle", agents: ["a6", "a7"])
render("C7-still-running", agents: ["a3", "a5"])
render("O5-with-idle", agents: ["a2", "a5"], height: 300) { $0.mode = .open }
render("C2-working", agents: ["a2", "a3"])
render("C3-needs-you")
render("C4-done", agents: ["a4"])
render("C4b-working-and-done", agents: ["a2", "a3", "a4"])
render("C6-hover", agents: ["a2", "a3"]) { $0.hovering = true }
render("P1-permission", height: 260) { $0.mode = .peekAsk("a1") }
render("P2-finished", agents: ["a2", "a4"]) { $0.mode = .peekDone("a4") }
render("P3-connect", agents: ["a2", "a3"], height: 240, devices: [PendingDevice(id: "d1", name: "Pixel 8", platform: "android", expiresAt: (Date().timeIntervalSince1970 + 112) * 1000)]) {
    $0.mode = .connect("d1")
}
render("O1-agents", height: 440) { $0.mode = .open }
render("O2-new-agent", height: 300) {
    $0.mode = .open
    $0.tab = .compose
    $0.draft = "Turn the notch mockup into a SwiftUI spec"
}
render("O3-model-picker", height: 460) {
    $0.mode = .open
    $0.tab = .compose
    $0.menu = .model
    $0.draft = "Turn the notch mockup into a SwiftUI spec"
}
render("O4-empty", agents: [], height: 220) { $0.mode = .open }
render("D1-drop", height: 260) { $0.mode = .drop }
render("V1-dev-open", height: 440, dev: true) { $0.mode = .open }
render("X-plain-working", geometry: plain, height: 100)
render("X-plain-open", geometry: plain, height: 440) { $0.mode = .open }
