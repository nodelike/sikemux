import AppKit
@testable import NotchKit
import SwiftUI

// Renders each state of the island against demo agents, to compare with
// helpers/notch/design/screens.html: `swift run notch-snapshots <dir>`.
// `--bare` leaves out the backdrop, so the showcase can lay the island over its own wallpaper;
// `--scale 3` draws it at three pixels a point, `--fixture <file>` reads another device view, and `--faces`
// gives each agent a face from the core's catalogue, as if faces were on.

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
      ],
       "model": {"id": "model", "current": "opus", "choices": [
         {"value": "opus", "label": "Opus 5.5"}, {"value": "sonnet", "label": "Sonnet 5.5"}, {"value": "haiku", "label": "Haiku 4.5"}
       ]},
       "effort": {"id": "effort", "current": "high", "choices": [
         {"value": "low", "label": "Low"}, {"value": "medium", "label": "Medium"}, {"value": "high", "label": "High"}, {"value": "max", "label": "Max"}
       ]}},
      {"id": "codex", "provider": "codex", "label": "Codex", "permissionMode": "bypass", "configOptions": null},
      {"id": "opencode", "provider": "opencode", "label": "OpenCode", "permissionMode": "bypass", "configOptions": null},
      {"id": "grok", "provider": "grok", "label": "Grok", "permissionMode": "bypass", "configOptions": null}
    ],
    "palette": {},
    "backdrop": {"texture": false, "image": null}
  },
  "chats": [
    {"agentId": "a1", "name": "Otter", "provider": "claude", "cwd": "/Users/me/sikemux", "attentionRank": 0, "subagents": 0, "tasks": 0},
    {"agentId": "a2", "name": "Flint", "provider": "codex", "cwd": "/Users/me/server", "attentionRank": 5, "subagents": 3, "tasks": 0},
    {"agentId": "a3", "name": "Wren", "provider": "opencode", "cwd": "/Users/me/sikemux", "attentionRank": 6, "subagents": 0, "tasks": 0, "reconnecting": 3},
    {"agentId": "a4", "name": "Birch", "provider": "grok", "cwd": "/Users/me/sikemux-front", "attentionRank": 2, "subagents": 0, "tasks": 0},
    {"agentId": "a5", "name": "Heron", "provider": "claude", "title": "Profile the terminal flood", "cwd": "/Users/me/sikemux", "attentionRank": 7, "subagents": 0, "tasks": 2},
    {"agentId": "a9", "name": "Maple", "provider": "codex", "cwd": "/Users/me/server", "attentionRank": 1, "subagents": 0, "tasks": 0},
    {"agentId": "a10", "name": "Pine", "provider": "claude", "cwd": "/Users/me/server", "attentionRank": 3, "subagents": 0, "tasks": 0},
    {"agentId": "a11", "name": "Moss", "provider": "claude", "cwd": "/Users/me/sikemux", "attentionRank": 4, "subagents": 0, "tasks": 0}
  ],
  "sessions": [
    {"id": 7, "running": true, "project": "p-front", "agentId": "t1", "agentName": "Cedar", "agentType": "claude", "title": "Tighten the rail spacing", "attentionRank": 8}
  ],
  "attentions": [
    {"id": "p1", "agentId": "a1",
     "choices": {"allowOnce": "o1", "allowAlways": "o2", "reject": "o3", "command": "pnpm test --filter core -- --runInBand", "title": "Run tests"}}
  ],
  "faceStyle": {"motion": true},
  "inbox": {"working": 2, "rollup": {"subagents": 3, "tasks": 2}, "agents": [
    {"agentId": "a1", "title": "Fix the login flake", "group": "blocked", "subagents": 0, "tasks": 0},
    {"agentId": "a9", "title": "Move the ledger import", "group": "blocked", "subagents": 0, "tasks": 0},
    {"agentId": "a4", "title": "Bump Astro to 6", "group": "done", "subagents": 0, "tasks": 0, "nextTrigger": IN(1080)},
    {"agentId": "a10", "title": "Rotate the signing keys", "group": "done", "subagents": 0, "tasks": 0},
    {"agentId": "a11", "title": "Tidy the changelog", "group": "done", "subagents": 0, "tasks": 0},
    {"agentId": "a2", "title": "Refactor the auth middleware", "group": "working", "subagents": 3, "tasks": 0},
    {"agentId": "a3", "title": "Draft the notch spec", "group": "working", "subagents": 0, "tasks": 0},
    {"agentId": "a5", "title": "Profile the terminal flood", "group": "idle", "subagents": 0, "tasks": 2},
    {"agentId": "t1", "title": "Tighten the rail spacing", "group": "idle", "subagents": 0, "tasks": 0, "nextTrigger": IN(10800)}
  ], "items": [
    {"id": "permission:a1:p1", "tier": "needs", "kind": "permission", "agentId": "a1", "name": "Otter", "cli": "claude", "project": "p-sikemux",
     "title": "Fix the login flake", "what": "Needs your permission", "detail": "pnpm test --filter core -- --runInBand", "at": AGO(134), "requestId": "p1"},
    {"id": "question:q1", "tier": "needs", "kind": "question", "agentId": "a9", "name": "Maple", "cli": "codex", "project": "p-server",
     "title": "Move the ledger import", "what": "Asks you", "detail": "Should amounts be stored in cents or as decimals? The API sends decimals but the ledger table uses integers.",
     "at": AGO(1), "questionId": "q1"},
    {"id": "agent:a4", "tier": "review", "kind": "changes", "agentId": "a4", "name": "Birch", "cli": "grok", "project": "p-front",
     "title": "Bump Astro to 6", "what": "Finished, with changes to review", "at": AGO(70)},
    {"id": "agent:a10", "tier": "finished", "kind": "failed", "agentId": "a10", "name": "Pine", "cli": "claude", "project": "p-server",
     "title": "Rotate the signing keys", "what": "Ran into a problem", "detail": "The keychain refused the new key", "at": AGO(300)},
    {"id": "agent:a11", "tier": "finished", "kind": "finished", "agentId": "a11", "name": "Moss", "cli": "claude", "project": "p-sikemux",
     "title": "Tidy the changelog", "what": "Finished", "at": AGO(900)},
    {"id": "agent:a2", "tier": "progress", "kind": "working", "agentId": "a2", "name": "Flint", "cli": "codex", "project": "p-server",
     "title": "Refactor the auth middleware", "what": "Working", "at": AGO(252)},
    {"id": "agent:a3", "tier": "progress", "kind": "working", "agentId": "a3", "name": "Wren", "cli": "opencode", "project": "p-sikemux",
     "title": "Draft the notch spec", "what": "Working", "at": AGO(48)}
  ]}
}
"""#

let arguments = CommandLine.arguments
func option(_ name: String) -> String? {
    arguments.firstIndex(of: name).flatMap { $0 + 1 < arguments.count ? arguments[$0 + 1] : nil }
}
let valued = Set(["--scale", "--fixture"].compactMap { name in option(name).map { _ in name } })
let paths = arguments.dropFirst().enumerated().filter { index, argument in
    !argument.hasPrefix("--") && !valued.contains(arguments[index])
}.map(\.element)
let outputDir = URL(fileURLWithPath: paths.first ?? "/tmp/notch-snapshots")
let bare = arguments.contains("--bare")
let scale = option("--scale").flatMap(Double.init).map { CGFloat($0) } ?? 1
let source = option("--fixture").map { try! String(contentsOfFile: $0, encoding: .utf8) } ?? fixture
try? FileManager.default.createDirectory(at: outputDir, withIntermediateDirectories: true)

let fontsDir = URL(fileURLWithPath: #filePath).deletingLastPathComponent().appendingPathComponent("../../Fonts").standardized.path
Theme.registerFonts(in: fontsDir)
_ = NSApplication.shared
NSApp.setActivationPolicy(.prohibited)

let now = Int(Date().timeIntervalSince1970 * 1000)
let dated = try! NSRegularExpression(pattern: #"(AGO|IN)\((\d+)\)"#).matches(in: source, range: NSRange(source.startIndex..., in: source))
    .reversed().reduce(source) { text, match in
        let ahead = (source as NSString).substring(with: match.range(at: 1)) == "IN"
        let seconds = Int((source as NSString).substring(with: match.range(at: 2)))!
        return (text as NSString).replacingCharacters(in: match.range, with: String(now + (ahead ? 1 : -1) * seconds * 1000))
    }
let decoded = try! JSONDecoder().decode(DeviceView.self, from: Data(dated.utf8))
AgentCatalog.names = ["claude": "Claude", "codex": "Codex", "opencode": "OpenCode", "grok": "Grok"]
/// The first pack's shapes in the core's catalogue, one to each agent in turn.
func catalogueFaces() -> [AgentFace] {
    let path = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
        .appendingPathComponent("../../../../crates/sikemux-core/src/assets/faces.json").standardized
    struct Catalogue: Decodable {
        struct Pack: Decodable {
            struct Shape: Decodable { let id: String; let box: String }
            let id: String
            let shapes: [Shape]
            let colors: Int
        }
        let packs: [Pack]
    }
    guard let pack = (try? JSONDecoder().decode(Catalogue.self, from: Data(contentsOf: path)))?.packs.first else { return [] }
    return pack.shapes.enumerated().map { index, shape in
        AgentFace(pack: pack.id, shape: shape.id, color: index % pack.colors, box: shape.box)
    }
}
let faces = arguments.contains("--faces") ? catalogueFaces() : []
let agents = faces.isEmpty ? decoded.inbox.agents : decoded.inbox.agents.enumerated().map { index, agent in
    AgentShown(agentId: agent.agentId, title: agent.title, group: agent.group, subagents: agent.subagents, tasks: agent.tasks, nextTrigger: agent.nextTrigger, calledAt: agent.calledAt,
               face: faces[index % faces.count])
}
let full = DeviceView(
    workspace: decoded.workspace, sessions: decoded.sessions, chats: decoded.chats, attentions: decoded.attentions,
    inbox: Inbox(items: decoded.inbox.items, working: decoded.inbox.working, agents: agents, rollup: decoded.inbox.rollup),
    faceStyle: decoded.faceStyle
)

func view(keeping ids: Set<String>?) -> DeviceView {
    guard let ids else { return full }
    let items = full.inbox.items.filter { $0.agentId.map(ids.contains) ?? false }
    let agents = full.inbox.agents.filter { ids.contains($0.agentId) }
    return DeviceView(
        workspace: full.workspace,
        sessions: full.sessions.filter { $0.agentId.map(ids.contains) ?? false },
        chats: full.chats.filter { ids.contains($0.agentId) },
        attentions: full.attentions.filter { ids.contains($0.agentId) },
        inbox: Inbox(
            items: items, working: items.filter { $0.kind == .working }.count,
            agents: agents,
            rollup: AgentRollup(subagents: agents.reduce(0) { $0 + $1.subagents }, tasks: agents.reduce(0) { $0 + $1.tasks })
        ),
        faceStyle: full.faceStyle
    )
}

let questions = [AgentQuestion(
    id: "q1",
    text: "Should amounts be stored in cents or as decimals? The API sends decimals but the ledger table uses integers.",
    choices: [QuestionChoice(label: "Cents"), QuestionChoice(label: "Decimals")], viaNote: "Otter didn't know"
)]

let notch = NotchGeometry(notchWidth: 221, height: 38, centerX: 0)
let plain = NotchGeometry(notchWidth: 0, height: 24, centerX: 0)

func render(_ name: String, agents: Set<String>? = nil, geometry: NotchGeometry = notch, height: CGFloat = 140, dev: Bool = false,
            devices: [PendingDevice] = [], setup: (IslandModel) -> Void = { _ in })
{
    var options = Options()
    options.dev = dev
    let store = NotchStore(options: options)
    store.apply(view(keeping: agents))
    store.apply(questions)
    store.apply(devices, now: Date().addingTimeInterval(-8))
    let island = IslandModel(geometry: geometry)
    setup(island)
    let size = CGSize(width: 760, height: height)
    let root = ZStack(alignment: .top) {
        if !bare {
            LinearGradient(colors: [Color(hex: "#3b2a6e"), Color(hex: "#141024"), Color(hex: "#1f3b6b")], startPoint: .bottomLeading, endPoint: .topTrailing)
            Color.black.frame(height: 10).frame(maxHeight: .infinity, alignment: .top)
        }
        IslandView(store: store, island: island).padding(.top, bare ? 0 : 10)
    }
    .frame(width: size.width, height: size.height)
    let window = NSWindow(contentRect: CGRect(origin: .zero, size: size), styleMask: .borderless, backing: .buffered, defer: false)
    let host = NSHostingView(rootView: root)
    host.frame = CGRect(origin: .zero, size: size)
    window.contentView = host
    host.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.8))
    let bitmap: NSBitmapImageRep
    if scale == 1 {
        guard let cached = host.bitmapImageRepForCachingDisplay(in: host.bounds) else { return }
        host.cacheDisplay(in: host.bounds, to: cached)
        bitmap = cached
    } else {
        // A window offscreen draws one pixel a point, so a sharper picture is drawn without one.
        let image = MainActor.assumeIsolated {
            let renderer = ImageRenderer(content: root.environment(\.stillMarks, true))
            renderer.scale = scale
            return renderer.cgImage
        }
        guard let image else { return }
        bitmap = NSBitmapImageRep(cgImage: image)
    }
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
render("C7-subagents-and-shells", agents: ["a2", "a5"])
render("O5-finished", agents: ["a2", "a4", "a10", "a11"], height: 340) { $0.mode = .open }
render("C2-working", agents: ["a2", "a3"])
render("C3-needs-you")
render("C4-done", agents: ["a4", "a10", "a11"])
render("C4b-working-and-done", agents: ["a2", "a3", "a4"])
render("C6-hover", agents: ["a2", "a3"]) { $0.hovering = true }
render("P1-permission", height: 260) { $0.mode = .peekAsk("permission:a1:p1") }
render("P4-question", agents: ["a2", "a9"], height: 260) { $0.mode = .peekAsk("question:q1") }
render("P2-finished", agents: ["a2", "a4"]) { $0.mode = .peekDone("agent:a4") }
render("P3-connect", agents: ["a2", "a3"], height: 240, devices: [PendingDevice(id: "d1", name: "Pixel 8", platform: "android", expiresAt: now + 112_000)]) {
    $0.mode = .connect("d1")
}
render("O1-inbox", height: 520) { $0.mode = .open }
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
