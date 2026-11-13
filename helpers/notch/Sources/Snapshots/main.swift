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
      {"id": "p-sikemux", "name": "sikemux", "path": "/Users/me/sikemux", "chief": "c1", "agents": 4},
      {"id": "p-server", "name": "server", "path": "/Users/me/server"},
      {"id": "p-front", "name": "sikemux-front", "path": "/Users/me/sikemux-front"},
      {"id": "p-storefront", "name": "storefront", "path": "/Users/me/storefront"}
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
  "attentions": [
    {"id": "p1", "agentId": "a1",
     "choices": {"allowOnce": "o1", "allowAlways": "o2", "reject": "o3", "command": "pnpm test --filter core -- --runInBand", "title": "Run tests"}}
  ],
  "faceStyle": {"motion": true},
  "inbox": {"working": 2, "rollup": {"subagents": 3, "tasks": 2}, "agents": [
    {"agentId": "c1", "chief": true, "name": "Chief", "cli": "claude", "project": "p-sikemux", "projectName": "sikemux", "title": "Friendlier welcome screen", "titled": true, "turn": {"state": "working", "since": AGO(40)}, "group": "working", "failed": false, "subagents": 0, "tasks": 0},
    {"agentId": "a1", "name": "Otter", "cli": "claude", "project": "p-sikemux", "projectName": "sikemux", "title": "Fix the login flake", "titled": true, "turn": {"state": "needsYou", "since": AGO(134), "call": {"id": "p1", "question": "Run pnpm test?", "waitingSince": AGO(134), "count": 1}}, "group": "blocked", "failed": false, "subagents": 0, "tasks": 0},
    {"agentId": "a9", "name": "Maple", "cli": "codex", "project": "p-server", "projectName": "server", "title": "Move the ledger import", "titled": true, "turn": {"state": "needsYou", "since": AGO(60), "call": {"id": "q1", "question": "Cents or decimals?", "waitingSince": AGO(60), "count": 1}}, "group": "blocked", "failed": false, "subagents": 0, "tasks": 0},
    {"agentId": "a4", "name": "Birch", "cli": "grok", "project": "p-front", "projectName": "sikemux-front", "title": "Bump Astro to 6", "titled": true, "turn": {"state": "done", "since": AGO(70), "done": {"changes": "landed", "checks": "pass"}}, "group": "done", "failed": false, "subagents": 0, "tasks": 0, "nextTrigger": IN(1080)},
    {"agentId": "a10", "name": "Pine", "cli": "claude", "project": "p-server", "projectName": "server", "title": "Rotate the signing keys", "titled": true, "turn": {"state": "failed", "since": AGO(300), "failed": {"why": "other", "said": "The keychain refused the new key"}}, "group": "done", "failed": false, "subagents": 0, "tasks": 0},
    {"agentId": "a11", "name": "Moss", "cli": "claude", "project": "p-sikemux", "projectName": "sikemux", "title": "Tidy the changelog", "titled": true, "turn": {"state": "stopped", "since": AGO(900), "stopped": {"left": "Tidy the 0.6 section"}}, "group": "done", "failed": false, "subagents": 0, "tasks": 0},
    {"agentId": "a20", "name": "Moss", "cli": "claude", "project": "p-sikemux", "projectName": "sikemux", "title": "Tidy the changelog", "titled": true, "turn": {"state": "waiting", "since": AGO(903), "wait": {"kind": "line", "what": "cargo test", "startedAt": AGO(90), "place": 2, "expectedMs": 240000}}, "group": "idle", "failed": false, "subagents": 0, "tasks": 0, "line": {"what": "cargo test", "place": 2, "expectedMs": 240000}},
    {"agentId": "a2", "name": "Flint", "cli": "codex", "project": "p-server", "projectName": "server", "title": "Refactor the auth middleware", "titled": true, "turn": {"state": "working", "since": AGO(252)}, "group": "working", "failed": false, "subagents": 3, "tasks": 0},
    {"agentId": "a3", "name": "Wren", "cli": "opencode", "project": "p-sikemux", "projectName": "sikemux", "reconnecting": 3, "title": "Draft the notch spec", "titled": true, "turn": {"state": "working", "since": AGO(48)}, "group": "working", "failed": false, "subagents": 0, "tasks": 0},
    {"agentId": "a5", "name": "Heron", "cli": "claude", "project": "p-sikemux", "projectName": "sikemux", "title": "Profile the terminal flood", "titled": true, "turn": {"state": "waiting", "since": AGO(180), "wait": {"kind": "ci", "what": "CI on the flood fix", "startedAt": AGO(180), "expectedMs": 420000}}, "group": "idle", "failed": false, "subagents": 0, "tasks": 2},
    {"agentId": "a12", "name": "Kite", "cli": "claude", "project": "p-storefront", "projectName": "storefront", "title": "Ship the new welcome screen", "titled": true, "turn": {"state": "needsYou", "since": AGO(720), "call": {"id": "7", "question": "Ship the new welcome screen?", "waitingSince": AGO(720), "count": 2}}, "group": "blocked", "failed": false, "subagents": 0, "tasks": 0},
    {"agentId": "a13", "name": "Yuki", "cli": "claude", "project": "p-server", "projectName": "server", "title": "Add bank transfer to invoices", "titled": true, "turn": {"state": "needsYou", "since": AGO(120), "call": {"id": "w1", "question": "Ship bank transfer?", "waitingSince": AGO(120), "count": 1}}, "group": "blocked", "failed": false, "subagents": 0, "tasks": 0},
    {"agentId": "t1", "name": "Cedar", "cli": "claude", "project": "p-front", "projectName": "sikemux-front", "session": 7, "title": "Tighten the rail spacing", "titled": true, "turn": {"state": "waiting", "since": AGO(60), "wait": {"kind": "timer", "what": "Check the rail again", "startedAt": AGO(60), "until": IN(10800)}}, "group": "idle", "failed": false, "subagents": 0, "tasks": 0, "nextTrigger": IN(10800)}
  ], "items": [
    {"id": "permission:a1:p1", "tier": "needs", "kind": "permission", "heading": "Fix the login flake", "about": "Fix the login flake", "agentId": "a1", "name": "Otter", "cli": "claude", "project": "p-sikemux", "projectName": "sikemux",
     "title": "Fix the login flake", "what": "Needs your permission", "detail": "pnpm test --filter core -- --runInBand", "at": AGO(134), "requestId": "p1"},
    {"id": "question:q1", "tier": "needs", "kind": "question", "heading": "Move the ledger import", "about": "Move the ledger import", "agentId": "a9", "name": "Maple", "cli": "codex", "project": "p-server", "projectName": "server",
     "title": "Move the ledger import", "what": "Asks you", "detail": "Should amounts be stored in cents or as decimals? The API sends decimals but the ledger table uses integers.",
     "at": AGO(1), "questionId": "q1"},
    {"id": "ship:7", "tier": "needs", "kind": "ship", "heading": "Ship the new welcome screen", "about": "Ship the new welcome screen", "agentId": "a12", "name": "Kite", "cli": "claude", "project": "p-storefront", "projectName": "storefront",
     "title": "Ship the new welcome screen", "what": "Needs you", "detail": "Ship the new welcome screen to everyone?", "at": AGO(12)},
    {"id": "agent:a4", "tier": "review", "kind": "changes", "heading": "Bump Astro to 6", "about": "Bump Astro to 6", "agentId": "a4", "name": "Birch", "cli": "grok", "project": "p-front", "projectName": "sikemux-front",
     "title": "Bump Astro to 6", "what": "Finished, changed 3 files", "at": AGO(70)},
    {"id": "agent:a10", "tier": "finished", "kind": "failed", "heading": "Rotate the signing keys", "about": "Rotate the signing keys", "agentId": "a10", "name": "Pine", "cli": "claude", "project": "p-server", "projectName": "server",
     "title": "Rotate the signing keys", "what": "Ran into a problem", "detail": "The keychain refused the new key", "at": AGO(300)},
    {"id": "agent:a11", "tier": "finished", "kind": "finished", "heading": "Tidy the changelog", "about": "Tidy the changelog", "agentId": "a11", "name": "Moss", "cli": "claude", "project": "p-sikemux", "projectName": "sikemux",
     "title": "Tidy the changelog", "what": "Finished", "at": AGO(900)},
    {"id": "agent:a2", "tier": "progress", "kind": "working", "heading": "Refactor the auth middleware", "about": "Refactor the auth middleware", "agentId": "a2", "name": "Flint", "cli": "codex", "project": "p-server", "projectName": "server",
     "title": "Refactor the auth middleware", "what": "Working", "at": AGO(252)},
    {"id": "agent:a3", "tier": "progress", "kind": "working", "heading": "Draft the notch spec", "about": "Draft the notch spec", "agentId": "a3", "name": "Wren", "cli": "opencode", "project": "p-sikemux", "projectName": "sikemux",
     "title": "Draft the notch spec", "what": "Working", "at": AGO(48)},
    {"id": "host-alert:here:memory", "tier": "needs", "kind": "hostAlert", "heading": "mini", "what": "Memory 92% used", "at": AGO(124),
     "hostAlert": {"kind": "memory", "level": "danger", "since": AGO(124), "percent": 92}},
    {"id": "host-alert:forge:cpu", "tier": "needs", "kind": "hostAlert", "heading": "forge", "what": "CPU 97% busy", "at": AGO(1231), "host": "forge",
     "hostAlert": {"kind": "cpu", "level": "warn", "since": AGO(1231), "percent": 97}}
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
    AgentShown(agentId: agent.agentId, chief: agent.chief, name: agent.name, cli: agent.cli, title: agent.title, titled: agent.titled, session: agent.session, group: agent.group, turn: agent.turn, subagents: agent.subagents, tasks: agent.tasks, failed: agent.failed, nextTrigger: agent.nextTrigger, line: agent.line, calledAt: agent.calledAt,
               face: faces[index % faces.count], project: agent.project, projectName: agent.projectName, reconnecting: agent.reconnecting,
               snoozedUntil: nil, settledAt: nil)
}
/// The new welcome screen as staging shows it, drawn to a file for a call's evidence.
func welcomePicture(file: String, now: Bool = false) -> String {
    let ink = Color(hex: "#18181b"), dim = Color(hex: "#71717a")
    let today = VStack(alignment: .leading, spacing: 0) {
        Text("ACME").font(.system(size: 13, weight: .bold)).tracking(2).foregroundStyle(ink).padding(.bottom, 22)
        Text("Welcome to Acme Store").font(.system(size: 22, weight: .bold)).foregroundStyle(ink)
        Text("Sign in to continue.").font(.system(size: 14)).foregroundStyle(dim).padding(.top, 4).padding(.bottom, 16)
        Text("Email").font(.system(size: 14)).foregroundStyle(dim).frame(maxWidth: .infinity, alignment: .leading)
            .padding(10).background(RoundedRectangle(cornerRadius: 7).stroke(Color(hex: "#d4d4d0")))
        Text("Sign in").font(.system(size: 15, weight: .semibold)).foregroundStyle(.white).frame(maxWidth: .infinity)
            .padding(.vertical, 11).background(RoundedRectangle(cornerRadius: 7).fill(Color(hex: "#3f3f46")))
            .padding(.top, 10)
    }
    .padding(.horizontal, 26)
    .padding(.vertical, 22)
    .frame(width: 580, height: 330, alignment: .topLeading)
    .background(Color(hex: "#f6f6f4"))
    let page = now ? AnyView(today) : AnyView(VStack(alignment: .leading, spacing: 0) {
        Text("ACME").font(.system(size: 13, weight: .bold)).tracking(2).foregroundStyle(Color(hex: "#c2410c")).padding(.bottom, 16)
        HStack(spacing: 18) {
            Circle().fill(Color(hex: "#fdba74")).frame(width: 34, height: 34)
            Circle().fill(Color(hex: "#f9a8d4")).frame(width: 26, height: 26)
            Circle().fill(Color(hex: "#a7f3d0")).frame(width: 20, height: 20)
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 18)
        .background(RoundedRectangle(cornerRadius: 12).fill(Color(hex: "#fde7d4")))
        .padding(.bottom, 18)
        Text("Hi! Let's find something you'll love.").font(.system(size: 22, weight: .bold)).foregroundStyle(ink)
        Text("Look around first. No account needed.").font(.system(size: 14)).foregroundStyle(dim).padding(.top, 4).padding(.bottom, 16)
        Text("Get started").font(.system(size: 15, weight: .semibold)).foregroundStyle(.white).frame(maxWidth: .infinity)
            .padding(.vertical, 11).background(Capsule().fill(Color(hex: "#292524")))
        Text("I have an account").font(.system(size: 13)).foregroundStyle(Color(hex: "#c2410c")).frame(maxWidth: .infinity).padding(.top, 10)
    }
    .padding(.horizontal, 26)
    .padding(.vertical, 22)
    .frame(width: 580, height: 330, alignment: .topLeading)
    .background(Color(hex: "#fffaf5")))
    let path = NSTemporaryDirectory() + "notch-snapshot-\(file).png"
    let image = MainActor.assumeIsolated { () -> CGImage? in
        let renderer = ImageRenderer(content: page)
        renderer.scale = 2
        return renderer.cgImage
    }
    if let image { try? NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: path)) }
    return path
}

/// An invoice page with its new way to pay named `label`, as a call's picture shows a page, drawn to a file.
func invoicePicture(_ label: String, file: String, tall: Bool = false) -> String {
    let ink = Color(hex: "#18181b"), dim = Color(hex: "#71717a")
    let page = VStack(alignment: .leading, spacing: 0) {
        Text("$2,480.00").font(.system(size: 34, weight: .bold)).foregroundStyle(ink)
        Text("Northwind Studio · due 18 Nov").font(.system(size: 14)).foregroundStyle(dim).padding(.bottom, 16)
        HStack(spacing: 6) {
            Text("Card").frame(maxWidth: .infinity).padding(.vertical, 8).foregroundStyle(dim)
            Text(label).frame(maxWidth: .infinity).padding(.vertical, 8).background(RoundedRectangle(cornerRadius: 6).fill(.white)).foregroundStyle(ink)
        }
        .font(.system(size: 14, weight: .semibold))
        .padding(4)
        .background(RoundedRectangle(cornerRadius: 8).fill(Color(hex: "#ebeae6")))
        .padding(.bottom, 14)
        Text("Account name").font(.system(size: 12, weight: .semibold)).foregroundStyle(dim).padding(.bottom, 5)
        Text("Acme Ltd").font(.system(size: 14)).foregroundStyle(ink).frame(maxWidth: .infinity, alignment: .leading)
            .padding(10).background(RoundedRectangle(cornerRadius: 7).stroke(Color(hex: "#d4d4d0")))
        if tall {
            Text("IBAN").font(.system(size: 12, weight: .semibold)).foregroundStyle(dim).padding(.top, 10).padding(.bottom, 5)
            Text("GB29 NWBK 6016 1331 9268 19").font(.system(size: 14)).foregroundStyle(ink).frame(maxWidth: .infinity, alignment: .leading)
                .padding(10).background(RoundedRectangle(cornerRadius: 7).stroke(Color(hex: "#d4d4d0")))
        }
    }
    .padding(.horizontal, 26)
    .padding(.vertical, 22)
    .frame(width: 580, height: tall ? 330 : 270, alignment: .topLeading)
    .background(Color(hex: "#f6f6f4"))
    let path = NSTemporaryDirectory() + "notch-snapshot-\(file).png"
    let image = MainActor.assumeIsolated { () -> CGImage? in
        let renderer = ImageRenderer(content: page)
        renderer.scale = 2
        return renderer.cgImage
    }
    if let image { try? NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: path)) }
    return path
}

func asker(_ id: String, _ name: String, _ cli: String) -> CallAsker { CallAsker(agentId: id, name: name, cli: cli, chief: nil) }

/// The calls the demo agents put to the person: Maple's question, naming and font, Kite's two ships to production,
/// each with a picture where it has one.
let demoCalls = [
    PersonCall(id: "q1", project: "/Users/me/server", kind: .taste, asker: asker("a9", "Maple", "codex"), joined: nil,
               question: "Should amounts be stored in cents or as decimals?",
               context: "The API sends decimals but the ledger table uses integers.", assumption: nil,
               options: [CallOption(id: "1", label: "Cents", detail: nil, picture: nil), CallOption(id: "2", label: "Decimals", detail: nil, picture: nil)],
               pick: "1", against: "every import converts what the API sends.", evidence: nil, action: nil, brief: nil, place: nil,
               canHandBack: true, multi: nil, secret: nil, raisedAt: now - 60_000, state: .open, answer: nil, outcome: nil),
    PersonCall(id: "7", project: "/Users/me/storefront", kind: .production, asker: asker("a12", "Kite", "claude"), joined: nil,
               question: "Ship the new welcome screen to everyone?",
               context: "You asked for a friendlier welcome screen. It's on staging, and sign-ups there held at 4.1% over two days.", assumption: nil,
               options: [CallOption(id: "all", label: "Ship it to everyone", detail: nil, picture: nil),
                         CallOption(id: "half", label: "Ship to half first", detail: nil, picture: nil),
                         CallOption(id: "wait", label: "Not yet", detail: nil, picture: nil)],
               pick: "all", against: "step 2 still has the old copy.",
               evidence: [CallEvidence(what: "Now", picture: welcomePicture(file: "welcome-now", now: true)),
                          CallEvidence(what: "New", picture: welcomePicture(file: "welcome"))],
               action: "vercel deploy --prod", brief: nil,
               place: CallPlace(id: "production", name: "production"), canHandBack: false, multi: nil, secret: nil, raisedAt: now - 720_000, state: .open, answer: nil, outcome: nil),
    PersonCall(id: "m1", project: "/Users/me/storefront", kind: .money, asker: asker("a12", "Kite", "claude"), joined: nil,
               question: "Buy the display font, $40?", context: "The welcome screen's heading uses it. The licence covers the web and the app for good.", assumption: nil,
               options: [CallOption(id: "buy", label: "Buy it", detail: "$40 once", picture: nil),
                         CallOption(id: "free", label: "Use a free font instead", detail: nil, picture: nil)],
               pick: "buy", against: "a free font is close enough that most people won't see the difference.", evidence: nil, action: nil, brief: nil,
               place: nil, canHandBack: true, multi: nil, secret: nil, raisedAt: now - 180_000, state: .open, answer: nil, outcome: nil),
    PersonCall(id: "w1", project: "/Users/me/server", kind: .production, asker: asker("a13", "Yuki", "claude"), joined: nil,
               question: "Ship bank transfer on invoices to everyone?", context: nil, assumption: nil,
               options: [CallOption(id: "allow", label: "Ship it to everyone", detail: nil, picture: nil),
                         CallOption(id: "tenth", label: "Ship to a tenth of customers first", detail: nil, picture: nil),
                         CallOption(id: "refuse", label: "Not yet", detail: nil, picture: nil)],
               pick: "allow", against: "the bank's notice of payment has only been seen in its sandbox.",
               evidence: [CallEvidence(what: "The invoice page", picture: invoicePicture("Bank transfer", file: "invoice", tall: true))],
               action: "vercel deploy --prod",
               brief: CallBrief(tried: "@Wren built it and @Pine checked the totals.",
                                shows: "On staging since Tuesday, 41 test invoices paid and none failed.",
                                cannotUndo: "If it is rolled back, invoices already paid by transfer stay paid."),
               place: CallPlace(id: "production", name: "production"), canHandBack: false, multi: nil, secret: nil,
               raisedAt: now - 120_000, state: .open, answer: nil, outcome: nil),
    PersonCall(id: "t1", project: "/Users/me/server", kind: .taste, asker: asker("a9", "Maple", "codex"), joined: nil,
               question: "What do we call the new way to pay?", context: "It sits beside Card on every invoice. Testers asked when the money arrives.", assumption: nil,
               options: [CallOption(id: "1", label: "Bank transfer (2–3 days)", detail: nil, picture: invoicePicture("Bank transfer (2–3 days)", file: "name-1")),
                         CallOption(id: "2", label: "Pay by bank", detail: nil, picture: invoicePicture("Pay by bank", file: "name-2")),
                         CallOption(id: "3", label: "Bank transfer", detail: nil, picture: invoicePicture("Bank transfer", file: "name-3"))],
               pick: "1", against: "it is the longest, and on a phone it takes two lines beside Card.", evidence: nil, action: nil, brief: nil,
               place: nil, canHandBack: true, multi: nil, secret: nil, raisedAt: now - 3_600_000, state: .open, answer: nil, outcome: nil),
]

/// Fern's calls, which it goes on from: one about money with options, one for access without. Only the states that show
/// them keep Fern.
let goingOnCalls = [
    PersonCall(id: "g1", project: "/Users/me/server", kind: .money, asker: asker("a14", "Fern", "claude"), joined: nil,
               question: "Pay for the error tracker's team plan?", context: "The free plan keeps a week of errors; the team plan keeps ninety days.",
               assumption: "the free plan",
               options: [CallOption(id: "team", label: "Pay for the team plan", detail: "$26 a month", picture: nil),
                         CallOption(id: "free", label: "Stay on the free plan", detail: nil, picture: nil)],
               pick: "team", against: "a week is enough while the server has few users.", evidence: nil, action: nil, brief: nil,
               place: nil, canHandBack: true, multi: nil, secret: nil, raisedAt: now - 240_000, state: .open, answer: nil, outcome: nil),
    PersonCall(id: "g2", project: "/Users/me/server", kind: .access, asker: asker("a14", "Fern", "claude"), joined: nil,
               question: "Which Stripe key do the staging payment tests take?",
               context: "The tests charge test cards on staging before bank transfer ships.",
               assumption: "they run against the local mock until there is one",
               options: [CallOption(id: "skip", label: "Skip, use the local mock", detail: nil, picture: nil)], pick: nil,
               against: nil, evidence: nil,
               action: nil, brief: nil, place: nil, canHandBack: false, multi: nil, secret: nil, raisedAt: now - 60_000, state: .open,
               answer: nil, outcome: nil),
]

/// The workspace with each project standing as the core would put it, for the agents and calls shown.
/// A call the sikemux project's chief asks itself, which wears Sikemux's S wherever the island draws its asker.
let chiefCalls = [
    PersonCall(id: "c1q", project: "/Users/me/sikemux", kind: .taste,
               asker: CallAsker(agentId: "c1", name: "Chief", cli: "claude", chief: true), joined: nil,
               question: "Should the welcome screen say hello by name?", context: "Sign-up already has the name; the screen greets everyone the same today.",
               assumption: nil,
               options: [CallOption(id: "name", label: "Greet them by name", detail: nil, picture: nil),
                         CallOption(id: "same", label: "Keep one greeting", detail: nil, picture: nil)],
               pick: "name", against: "a name typed in a hurry reads back wrong on the first screen.", evidence: nil, action: nil,
               brief: nil, place: nil, canHandBack: true, multi: nil, secret: nil, raisedAt: now - 180_000, state: .open,
               answer: nil, outcome: nil),
]

func standing(_ workspace: Workspace, agents: [AgentShown], calls: [PersonCall]) -> Workspace {
    let projects = workspace.projects.map { project in
        let open = calls.filter { $0.project == project.path && $0.state == .open }.count
        let states = agents.filter { $0.project == project.id }.compactMap { $0.turn?.state }
        let standing: ProjectStanding =
            open > 0 || states.contains(.needsYou) ? .needsYou
            : states.contains(where: { $0 == .working || $0 == .waiting }) ? .atWork
            : .atRest
        return ProjectInfo(id: project.id, name: project.name, path: project.path, chief: project.chief,
                           agents: project.agents, standing: standing, calls: open > 0 ? open : nil)
    }
    return Workspace(projects: projects, launchers: workspace.launchers)
}

let full = DeviceView(
    workspace: standing(decoded.workspace, agents: agents, calls: (decoded.calls ?? []) + demoCalls),
    attentions: decoded.attentions,
    inbox: Inbox(items: decoded.inbox.items.filter { $0.kind != .hostAlert }, working: decoded.inbox.working, agents: agents, rollup: decoded.inbox.rollup),
    calls: (decoded.calls ?? []) + demoCalls, faceStyle: decoded.faceStyle, load: nil
)

let overloaded = HostLoad(overloaded: true, loadAverage: 180, cores: 10, builds: [
    AgentBuild(agentId: "a2", command: "xcodebuild -scheme App"), AgentBuild(agentId: "a3", command: "xcodebuild -scheme Widgets"),
    AgentBuild(agentId: "a5", command: "cargo build"),
])

func view(keeping ids: Set<String>?, load: HostLoad? = nil) -> DeviceView {
    guard let ids else { return full }
    let items = decoded.inbox.items.filter { $0.agentId.map(ids.contains) ?? ids.contains($0.id) }
    let agents = full.inbox.agents.filter { ids.contains($0.agentId) }
    let calls = ((full.calls ?? []) + goingOnCalls + chiefCalls).filter { ids.contains($0.asker.agentId) }
    return DeviceView(
        workspace: standing(full.workspace, agents: agents, calls: calls),
        attentions: full.attentions.filter { ids.contains($0.agentId) },
        inbox: Inbox(
            items: items, working: items.filter { $0.kind == .working }.count,
            agents: agents,
            rollup: AgentRollup(subagents: agents.reduce(0) { $0 + $1.subagents }, tasks: agents.reduce(0) { $0 + $1.tasks })
        ),
        calls: calls, faceStyle: full.faceStyle, load: load
    )
}

let questions = [AgentQuestion(
    id: "q1",
    text: "Should amounts be stored in cents or as decimals? The API sends decimals but the ledger table uses integers.",
    choices: [QuestionChoice(label: "Cents"), QuestionChoice(label: "Decimals")], viaNote: "@Otter raised this"
)]

let notch = NotchGeometry(notchWidth: 221, height: 38, centerX: 0)
let plain = NotchGeometry(notchWidth: 0, height: 24, centerX: 0)

func render(_ name: String, agents: Set<String>? = nil, load: HostLoad? = nil, geometry: NotchGeometry = notch, height: CGFloat = 140,
            dev: Bool = false, devices: [PendingDevice] = [], card: (NotchStore) -> Void = { _ in },
            setup: (IslandModel) -> Void = { _ in })
{
    var options = Options()
    options.dev = dev
    let store = NotchStore(options: options)
    store.apply(view(keeping: agents, load: load))
    store.apply(questions)
    store.apply(devices, now: Date().addingTimeInterval(-8))
    card(store)
    let island = IslandModel(geometry: geometry)
    setup(island)
    let size = CGSize(width: 760, height: height)
    island.canvas = CGRect(origin: .zero, size: CGSize(width: size.width, height: size.height - 10))
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
render("O6-host-alerts", agents: ["a2", "a4", "a11", "host-alert:here:memory", "host-alert:forge:cpu"], height: 320) { $0.mode = .open }
render("O7-mac-busy", agents: ["a2", "a20", "a4"], load: overloaded, height: 300) { $0.mode = .open }
render("C2-working", agents: ["a2", "a3"])
render("C3-needs-you")
render("C4-done", agents: ["a4", "a10", "a11"])
render("C4b-working-and-done", agents: ["a2", "a3", "a4"])
render("C6-hover", agents: ["a2", "a3"]) { $0.hovering = true }
render("C3b-no-calls", agents: ["a1", "a2", "a3"])
render("P1-permission", height: 260) { $0.mode = .peekAsk("permission:a1:p1") }
render("P4-call", agents: ["a2", "a9"], height: 380) { $0.mode = .call("q1") }
render("P5-production-call", agents: ["a2", "a12"], height: 640) { $0.mode = .call("7") }
render("P6-weighty-call", agents: ["a2", "a13"], height: 640) { $0.mode = .call("w1") }
render("P7-taste-call", agents: ["a2", "a9"], height: 560) { $0.mode = .call("t1") }
render("P8-own-words", agents: ["a2", "a9"], height: 380, card: { store in
    store.drafts["q1"]?.chosen = []
    store.drafts["q1"]?.typed = "Cents, and convert on import"
}) { $0.mode = .call("q1") }
render("P10-money-call", agents: ["a2", "a12"], height: 420) { $0.mode = .call("m1") }
render("P11-going-on-call", agents: ["a2", "a14"], height: 460) { $0.mode = .call("g1") }
render("P12-access-going-on", agents: ["a2", "a14"], height: 320) { $0.mode = .call("g2") }
render("P13-chief-call", agents: ["c1", "a2"], height: 420) { $0.mode = .call("c1q") }
render("P9-what-runs", agents: ["a2", "a12"], height: 720, card: { $0.drafts["7"]?.actionOpen = true }) { $0.mode = .call("7") }
render("P2-finished", agents: ["a2", "a4"]) { $0.mode = .peekDone("agent:a4") }
render("P3-connect", agents: ["a2", "a3"], height: 240, devices: [PendingDevice(id: "d1", name: "Pixel 8", platform: "android", expiresAt: now + 112_000)]) {
    $0.mode = .connect("d1")
}
render("O1-inbox", height: 720) { $0.mode = .open }
render("O1b-list-call", agents: ["a1", "a2", "a9", "a12"], height: 900) {
    $0.mode = .open
    $0.listCall = "t1"
}
render("O1e-list-chief-call", agents: ["c1", "a1", "a2", "a9"], height: 900) {
    $0.mode = .open
    $0.listCall = "t1"
}
render("O1c-list-weighty-call", agents: ["a1", "a2", "a13"], height: 900, card: { $0.drafts["w1"]?.actionOpen = true }) {
    $0.mode = .open
    $0.listCall = "w1"
}
render("O1d-list-call-short-screen", agents: ["a1", "a2", "a13"], height: 620, card: { $0.drafts["w1"]?.actionOpen = true }) {
    $0.mode = .open
    $0.listCall = "w1"
}
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
