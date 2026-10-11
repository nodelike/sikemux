import CoreGraphics
import FBSimulatorControl
import Foundation

@main
struct SikemuxSim {
    static func main() async {
        if CommandLine.arguments.contains("--version") {
            print("sikemux-sim 1")
            return
        }
        let simulators = Simulators()
        if CommandLine.arguments.contains("--probe") {
            await probe(simulators)
            return
        }
        let lines = AsyncStream<String> { continuation in
            Thread.detachNewThread {
                while let line = readLine() { continuation.yield(line) }
                continuation.finish()
            }
        }
        for await line in lines {
            switch Request.read(line) {
            case let .success(request) where Self.input.contains(request.type):
                lanes.run(on: request.udid) { await respond(to: request, with: simulators) }
            case let .success(request):
                Task { await respond(to: request, with: simulators) }
            case let .failure(unreadable):
                Output.failure(unreadable.id, "protocol", unreadable.message)
            }
        }
    }

    /// What a finger or a key does on a device, kept in the order it arrived, one at a time per device.
    /// A live `touch` stays out: it joins a gesture that is already holding the device's input.
    static let input: Set<String> = ["tap", "swipe", "touchPath", "touch2Path", "key", "text", "button"]
    static let lanes = InputLanes()

    static func respond(to request: Request, with simulators: Simulators) async {
        do {
            Output.result(request.id, try await handle(request, with: simulators))
        } catch let failure as Failure {
            Output.failure(request.id, failure.reason, failure.message)
        } catch {
            Output.failure(request.id, "simulator", Failure.describe(error))
        }
    }

    static func handle(_ request: Request, with simulators: Simulators) async throws -> [String: Any] {
        let udid = request.udid
        switch request.type {
        case "devices":
            return ["devices": try await simulators.devices()]
        case "runtimes":
            return ["runtimes": try await simulators.runtimes()]
        case "boot":
            try await simulators.boot(udid)
        case "shutdown":
            try await simulators.shutdown(udid)
        case "screenshot":
            let jpeg = request.format == "jpeg"
            let shot = try await simulators.screenshot(udid, jpeg: jpeg, pointSize: request.pointSize ?? false)
            if let given = request.path {
                let path = try writeScreenshot(shot.data, to: given)
                return ["path": path, "bytes": shot.data.count, "width": shot.width, "height": shot.height]
            }
            return [jpeg ? "jpeg" : "png": shot.data.base64EncodedString(), "width": shot.width, "height": shot.height]
        case "tree":
            return ["elements": try await simulators.tree(udid)]
        case "tap":
            let point = try await simulators.touchPoint(try require(request.x, request.y), on: udid)
            try await simulators.send(.tapAt(x: point.x, y: point.y, duration: request.duration ?? 0.05), to: udid)
        case "tapLabel":
            let frame = try await simulators.frame(of: try require(request.label, "label"), on: udid, wait: min(max(request.wait ?? 2, 0), 30))
            try await lanes.run(on: udid) {
                let center = try await simulators.touchPoint(CGPoint(x: frame.midX, y: frame.midY), on: udid)
                try await simulators.send(.tapAt(x: center.x, y: center.y, duration: 0.05), to: udid)
            }.value
            return ["frame": ["x": frame.minX, "y": frame.minY, "width": frame.width, "height": frame.height]]
        case "swipe":
            let from = try require(request.x, request.y), to = try require(request.toX, request.toY)
            let edge = try await simulators.edge(at: from, on: udid)
            let start = try await simulators.touchPoint(from, on: udid), end = try await simulators.touchPoint(to, on: udid)
            try await simulators.send(
                .swipe(start.x, yStart: start.y, xEnd: end.x, yEnd: end.y, delta: 0, duration: request.duration ?? 0.3, edge: edge), to: udid)
        case "touch":
            try await simulators.touch(try require(request.phase, "phase"), at: try require(request.x, request.y), on: udid)
        case "key":
            let code = try Keyboard.named(try require(request.key, "key"))
            try await simulators.send(.composite([.keyboard(direction: .down, keyCode: code), .keyboard(direction: .up, keyCode: code)]), to: udid)
        case "screen":
            return try await simulators.screen(udid)
        case "mask":
            return ["mask": try await simulators.mask(udid) ?? NSNull()]
        case "orientation":
            if let name = request.orientation {
                try await simulators.orient(udid, to: name)
                return ["orientation": name]
            }
            return ["orientation": try await simulators.orientation(udid)]
        case "touchPath":
            try await simulators.send(try touchPath(try await simulators.touchPoints(try require(request.points, "points"), on: udid)), to: udid)
        case "touch2Path":
            try await simulators.send(try twoFingerPath(try await simulators.touchPoints(try require(request.points, "points"), on: udid)), to: udid)
        case "text":
            var events: [SimulatorHIDEvent] = []
            for key in try Keyboard.keys(for: try require(request.text, "text")) {
                if key.shifted { events.append(.keyboard(direction: .down, keyCode: Keyboard.shift)) }
                events.append(.keyboard(direction: .down, keyCode: key.code))
                events.append(.keyboard(direction: .up, keyCode: key.code))
                if key.shifted { events.append(.keyboard(direction: .up, keyCode: Keyboard.shift)) }
                events.append(.delay(Keyboard.pause))
            }
            try await simulators.send(.composite(events), to: udid)
        case "button":
            let pressed = try button(try require(request.button, "button"))
            switch request.phase {
            case nil: try await simulators.send(.shortButtonPress(pressed), to: udid)
            case "down": try await simulators.send(.button(direction: .down, button: pressed), to: udid)
            case "up": try await simulators.send(.button(direction: .up, button: pressed), to: udid)
            default: throw Failure(reason: "badRequest", message: "A button's phase is down or up, or none for a press")
            }
        case "chrome":
            return ["chrome": try await simulators.chrome(udid) ?? NSNull()]
        case "logs":
            return try await simulators.logs(
                on: udid, process: request.process, after: request.after ?? 0, generation: request.generation, limit: min(max(request.limit ?? 200, 1), LogTail.capacity))
        case "stopLogs":
            try await simulators.stopLogs(on: udid, process: request.process)
        case "stream":
            return try await simulators.stream(on: udid, format: request.format ?? "h264", scale: request.scale)
        case "stopStream":
            try await simulators.stopStream(on: udid, format: request.format)
        case "install":
            return ["bundleId": try await simulators.install(try require(request.path, "path"), on: udid)]
        case "launch":
            let pid = try await simulators.launch(
                try require(request.bundleId, "bundleId"), arguments: request.arguments ?? [], environment: request.environment ?? [:], on: udid)
            return ["pid": pid]
        case "terminate":
            try await simulators.terminate(try require(request.bundleId, "bundleId"), on: udid)
        case "openUrl":
            guard let url = URL(string: try require(request.url, "url")) else {
                throw Failure(reason: "badRequest", message: "Not a URL: \(request.url ?? "")")
            }
            try await simulators.open(url, on: udid)
        default:
            throw Failure(reason: "protocol", message: "Unknown request \(request.type)")
        }
        return [:]
    }

    /// Writes to `given`, `~` meaning the home folder, into a folder that must already exist.
    static func writeScreenshot(_ data: Data, to given: String) throws -> String {
        let path = (given as NSString).expandingTildeInPath
        guard path.hasPrefix("/") else { throw Failure(reason: "badRequest", message: "A screenshot path must be absolute: \(given)") }
        var isFolder: ObjCBool = false
        let folder = (path as NSString).deletingLastPathComponent
        guard FileManager.default.fileExists(atPath: folder, isDirectory: &isFolder), isFolder.boolValue else {
            throw Failure(reason: "badRequest", message: "There is no folder \(folder) to save the screenshot in")
        }
        do {
            try data.write(to: URL(fileURLWithPath: path))
        } catch {
            throw Failure(reason: "badRequest", message: "Could not save the screenshot to \(path): \(Failure.describe(error))")
        }
        return path
    }

    private static func require<T>(_ value: T?, _ name: String) throws -> T {
        guard let value else { throw Failure(reason: "badRequest", message: "Missing \(name)") }
        return value
    }

    private static func require(_ x: Double?, _ y: Double?) throws -> CGPoint {
        guard let x, let y else { throw Failure(reason: "badRequest", message: "Missing x or y") }
        return CGPoint(x: x, y: y)
    }

    /// A finger put down at the first point, moved through the rest at their times, and lifted at the last.
    static func touchPath(_ points: [TouchPoint]) throws -> SimulatorHIDEvent {
        try path(points) { direction, point in .touch(direction: direction, x: point.x, y: point.y) }
    }

    static func twoFingerPath(_ points: [TouchPoint]) throws -> SimulatorHIDEvent {
        guard points.allSatisfy({ $0.x2 != nil && $0.y2 != nil }) else {
            throw Failure(reason: "badRequest", message: "Every point of a two-finger path needs x2 and y2")
        }
        return try path(points) { direction, point in
            .twoFingerTouch(direction: direction, finger1: CGPoint(x: point.x, y: point.y), finger2: CGPoint(x: point.x2!, y: point.y2!))
        }
    }

    private static func path(_ points: [TouchPoint], _ touch: (SimulatorHIDDirection, TouchPoint) -> SimulatorHIDEvent) throws -> SimulatorHIDEvent {
        guard let first = points.first, let last = points.last, points.count >= 2 else {
            throw Failure(reason: "badRequest", message: "A touch path needs at least two points")
        }
        guard zip(points, points.dropFirst()).allSatisfy({ $0.t <= $1.t }) else {
            throw Failure(reason: "badRequest", message: "A touch path's times must not go backwards")
        }
        var events = [touch(.down, first)]
        for (previous, point) in zip(points, points.dropFirst()) {
            if point.t > previous.t { events.append(.delay(point.t - previous.t)) }
            events.append(touch(.down, point))
        }
        events.append(touch(.up, last))
        return .composite(events)
    }

    private static func button(_ name: String) throws -> SimulatorHIDButton {
        let buttons: [String: SimulatorHIDButton] = [
            "home": .homeButton, "lock": .lock, "side": .sideButton, "siri": .siri, "volumeUp": .volumeUp, "volumeDown": .volumeDown,
            "action": .action,
        ]
        guard let button = buttons[name] else {
            throw Failure(reason: "badRequest", message: "Unknown button \(name). Use one of: \(buttons.keys.sorted().joined(separator: ", "))")
        }
        return button
    }

    /// Boots a device, reads its screen and taps, without the app; for checking a Mac or a CI runner.
    private static func probe(_ simulators: Simulators) async {
        func step(_ name: String, _ work: () async throws -> String) async -> Bool {
            let started = Date()
            do {
                let detail = try await work()
                print(String(format: "ok    %-12@ %6.0f ms  %@", name as NSString, Date().timeIntervalSince(started) * 1000, detail as NSString))
                return true
            } catch {
                print("fail  \(name)  \(Failure.describe(error))")
                return false
            }
        }
        var udid = CommandLine.arguments.drop { $0 != "--probe" }.dropFirst().first
        let passed = await step("devices") {
            let devices = try await simulators.devices()
            if udid == nil {
                udid = (devices.first { $0["state"] as? String == "booted" } ?? devices.first { ($0["name"] as? String)?.hasPrefix("iPhone") == true })?["udid"] as? String
            }
            return "\(devices.count) devices, using \(udid ?? "none")"
        }
        guard passed, let udid else { exit(1) }
        var ok = await step("boot") { try await simulators.boot(udid); return udid }
        ok = await step("screenshot") { "\(try await simulators.screenshot(udid).data.count) bytes" } && ok
        ok = await step("tree") { "\(String(describing: try await simulators.tree(udid)).count) characters" } && ok
        ok = await step("tap") { try await simulators.send(.tapAt(x: 1, y: 1), to: udid); return "at 1,1" } && ok
        exit(ok ? 0 : 1)
    }
}

/// One queue per device for input, so a person's touches and an agent's taps or typing never
/// interleave on the same screen. Different devices, and everything that is not input, run at once.
final class InputLanes: @unchecked Sendable {
    private let lock = NSLock()
    private var last: [String: Task<Void, Never>] = [:]

    /// Starts `work` once everything queued before it on the device has finished.
    @discardableResult
    func run<T: Sendable>(on udid: String?, _ work: @escaping @Sendable () async throws -> T) -> Task<T, Error> {
        lock.lock()
        defer { lock.unlock() }
        let key = udid ?? ""
        let previous = last[key]
        let task = Task<T, Error> {
            await previous?.value
            return try await work()
        }
        last[key] = Task { _ = try? await task.value }
        return task
    }
}
