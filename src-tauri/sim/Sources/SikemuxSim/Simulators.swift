import CoreGraphics
import CoreSimulator
import FBControlCore
import FBSimulatorControl
import Foundation

/// The device work. One per helper process, so CoreSimulator is loaded once and HID connections are reused.
actor Simulators {
    private var control: SimulatorControlBootstrap?
    private var touch: [String: SimulatorHID] = [:]
    private var tails: [String: (tail: LogTail, task: Task<Void, Never>)] = [:]
    private var streams: [String: FrameStream] = [:]
    /// The edge a live touch started at, kept for its moves and release, by device.
    private var touchEdges: [String: SimulatorHIDEdge] = [:]
    /// The latest live touch step sent to each device; the next one waits for it, so a move never
    /// overtakes its touch-down.
    private var touchSteps: [String: Task<Void, Error>] = [:]
    /// The way each device was last turned here, which sets the shape its touches are measured on.
    private var orientations: [String: String] = [:]

    func devices() throws -> [[String: Any]] {
        try set().allSimulators.map { simulator in
            [
                "udid": simulator.udid,
                "name": simulator.name,
                "state": simulator.state == .booted ? "booted" : simulator.state == .shutdown ? "shutdown" : "busy",
                "runtime": simulator.osVersion.name.rawValue,
                "model": simulator.deviceType.model.rawValue,
            ].merging(Self.screenSize(simulator).map { ["screen": ["width": $0.width, "height": $0.height, "scale": $0.scale]] } ?? [:]) { _, new in new }
        }
    }

    func runtimes() throws -> [[String: Any]] {
        _ = try set()
        return (control?.serviceContext.supportedRuntimes() ?? []).map { runtime in
            ["identifier": runtime.identifier, "name": runtime.name, "version": runtime.versionString, "available": runtime.available]
        }
    }

    func boot(_ udid: String?) async throws {
        let simulator = try find(udid)
        if simulator.state != .booted { try await simulator.lifecycle.boot(.default) }
        try await simulator.lifecycle.resolveUsable()
    }

    func shutdown(_ udid: String?) async throws {
        let simulator = try find(udid)
        touch[simulator.udid] = nil
        try await set().shutdown(simulator)
    }

    /// `pointSize` draws one pixel per point, a quarter of the pixels of a Retina screenshot.
    func screenshot(_ udid: String?, jpeg: Bool = false, pointSize: Bool = false) async throws -> (data: Data, width: Int, height: Int) {
        let simulator = try await booted(udid)
        let scale = pointSize ? Double(simulator.screenInfo?.scale ?? 1) : 1
        let configuration = ScreenshotConfiguration(
            encoding: jpeg ? .jpeg(quality: 0.8) : .png, scale: scale > 1 ? .factor(1 / scale) : .native)
        let shot = try await simulator.screenshot.take(configuration: configuration)
        return (shot.imageData, Int(shot.size.width), Int(shot.size.height))
    }

    /// One step of a touch the person is making live, sent in order after the step before it.
    func touch(_ phase: TouchPhase, at point: CGPoint, on udid: String?) async throws {
        let simulator = try await booted(udid)
        let edge: SimulatorHIDEdge
        if phase == .down {
            edge = edgeAt(point, of: simulator)
            touchEdges[simulator.udid] = edge
        } else {
            edge = touchEdges[simulator.udid] ?? .none
        }
        if phase == .up { touchEdges[simulator.udid] = nil }
        try await sendInOrder(.touch(direction: phase == .up ? .up : .down, x: point.x, y: point.y, edge: edge), to: simulator.udid)
    }

    func touch2(_ phase: TouchPhase, at first: CGPoint, and second: CGPoint, on udid: String?) async throws {
        let simulator = try await booted(udid)
        try await sendInOrder(.twoFingerTouch(direction: phase == .up ? .up : .down, finger1: first, finger2: second), to: simulator.udid)
    }

    func edge(at point: CGPoint, on udid: String?) async throws -> SimulatorHIDEdge {
        edgeAt(point, of: try await booted(udid))
    }

    func rotate(_ udid: String?, to name: String) async throws -> String {
        let orientations: [String: SimulatorDeviceOrientation] = [
            "portrait": .portrait, "portraitUpsideDown": .portraitUpsideDown, "landscapeLeft": .landscapeLeft, "landscapeRight": .landscapeRight,
        ]
        guard let orientation = orientations[name] else {
            throw Failure(reason: "badRequest", message: "Unknown orientation \(name). Use one of: \(orientations.keys.sorted().joined(separator: ", "))")
        }
        let simulator = try await booted(udid)
        try await simulator.orientation.set(orientation)
        self.orientations[simulator.udid] = name
        return name
    }

    func orientation(_ udid: String?) async throws -> String {
        let simulator = try await booted(udid)
        let name = "\(try await simulator.orientation.current().rawValue)"
        orientations[simulator.udid] = name
        return name
    }

    /// Draws the device around the screen as Xcode's own simulator window does, into PNGs at `chrome` and `mask`.
    func chrome(_ udid: String?, chrome: String, mask: String) throws -> [String: Any] {
        let simulator = try find(udid)
        guard let size = Self.screenSize(simulator) else {
            throw Failure(reason: "notFound", message: "\(simulator.name) has no screen to draw a device around")
        }
        let layout = try DeviceChrome.render(
            deviceType: simulator.deviceType.model.rawValue, screen: CGSize(width: size.width, height: size.height),
            chrome: URL(fileURLWithPath: chrome), mask: URL(fileURLWithPath: mask))
        return [
            "width": layout.size.width, "height": layout.size.height,
            "screen": ["x": layout.screen.minX, "y": layout.screen.minY, "width": layout.screen.width, "height": layout.screen.height],
        ]
    }

    func tree(_ udid: String?) async throws -> Any {
        let response = try await booted(udid).uiAutomation(backend: .accessibility)
            .describe(.frontmost, options: AccessibilityRequestOptions(format: .nested, enableLogging: false))
        return try JSONSerialization.jsonObject(with: JSONEncoder().encode(response.elements.elements))
    }

    /// Looks again every quarter second until `wait` runs out, since a label is often still on its way in
    /// just after a tap, a key press or a screen change.
    func frame(of label: String, on udid: String?, wait: TimeInterval) async throws -> CGRect {
        let automation = try await booted(udid).uiAutomation(backend: .accessibility)
        let deadline = Date().addingTimeInterval(wait)
        while true {
            do {
                return try await automation.frame(.marker(value: label, key: .label, depth: .max))
            } catch {
                guard Date() < deadline else { throw Failure(reason: "notFound", message: "\(error)") }
                try await Task.sleep(nanoseconds: 250_000_000)
            }
        }
    }

    private func sendInOrder(_ step: SimulatorHIDEvent, to udid: String) async throws {
        let previous = touchSteps[udid]
        let sent = Task {
            _ = await previous?.result
            try await self.send(step, to: udid)
        }
        touchSteps[udid] = sent
        try await sent.value
    }

    /// A touch starting at a side of the screen is tagged with it, so iOS treats it as the system
    /// gesture a finger there would make: home, back, Notification Center or Control Center.
    private func edgeAt(_ point: CGPoint, of simulator: Simulator) -> SimulatorHIDEdge {
        guard let size = Self.screenSize(simulator) else { return .none }
        let sideways = orientations[simulator.udid]?.hasPrefix("landscape") == true
        let (width, height) = sideways ? (size.height, size.width) : (size.width, size.height)
        let reach = 10.0
        if point.y >= height - reach { return .bottom }
        if point.y <= reach { return .top }
        if point.x <= reach { return .left }
        if point.x >= width - reach { return .right }
        return .none
    }

    /// The screen in points, the unit every coordinate here is in.
    private static func screenSize(_ simulator: Simulator) -> (width: Double, height: Double, scale: Double)? {
        guard let screen = simulator.screenInfo, screen.scale > 0 else { return nil }
        let scale = Double(screen.scale)
        return (Double(screen.widthPixels) / scale, Double(screen.heightPixels) / scale, scale)
    }

    func send(_ event: SimulatorHIDEvent, to udid: String?) async throws {
        let simulator = try await booted(udid)
        let hid: SimulatorHID
        if let connected = touch[simulator.udid] {
            hid = connected
        } else {
            hid = try await simulator.hid.connect()
            touch[simulator.udid] = hid
        }
        try await hid.send(event: event, logger: simulator.logger)
    }

    /// Starts following a device's log the first time it is asked for, filtered to one process if one is named.
    func logs(on udid: String?, process: String?, after cursor: Int, limit: Int) async throws -> [String: Any] {
        let simulator = try await booted(udid)
        let key = "\(simulator.udid) \(process ?? "")"
        if tails[key] == nil {
            let tail = LogTail()
            var arguments = ["--style", "compact"]
            if let process {
                arguments += ["--predicate", "process == \"\(process.replacingOccurrences(of: "\"", with: ""))\""]
            } else {
                // Apple's own frameworks log thousands of activity and debug-level entries a
                // second, many without a subsystem at all, so excluding com.apple.* alone still
                // lets those through; keeping only ordinary log messages at Notice level and
                // louder is what leaves an app's own logging.
                arguments += ["--type", "log", "--level", "info", "--predicate", "NOT (subsystem BEGINSWITH \"com.apple.\")"]
            }
            let operation = try await simulator.log.tail(arguments: arguments, consumer: tail.consumer)
            tail.attach(operation)
            let task = Task { _ = try? await operation.waitUntilCompleted() }
            tails[key] = (tail, task)
        }
        return tails[key]!.tail.read(after: cursor, limit: limit)
    }

    func stopLogs(on udid: String?, process: String?) throws {
        let key = "\(try find(udid).udid) \(process ?? "")"
        tails.removeValue(forKey: key)?.task.cancel()
    }

    /// One stream per device and format, shared by every viewer. H.264 sends a key frame each second so a
    /// late viewer starts within one; MJPEG is for a viewer whose H.264 decoder will not start.
    func stream(on udid: String?, format: String, fps: Int, scale: Double?) async throws -> [String: Any] {
        let simulator = try await booted(udid)
        let videoFormat: VideoStreamFormat
        switch format {
        case "h264": videoFormat = .compressedVideo(withCodec: .h264, transport: .annexB)
        case "mjpeg": videoFormat = .mjpeg(encoder: .allowSoftware)
        default: throw Failure(reason: "badRequest", message: "Unknown stream format \(format). Use h264 or mjpeg.")
        }
        let key = "\(simulator.udid) \(format)"
        if streams[key] == nil {
            let stream = try FrameStream()
            try await stream.listen()
            let configuration = VideoStreamConfiguration(
                format: videoFormat, framesPerSecond: fps, rateControl: nil, scaleFactor: scale, keyFrameRate: 1)
            stream.operation = try await simulator.videoStream.create(configuration: configuration, to: stream)
            streams[key] = stream
        }
        let stream = streams[key]!
        var answer: [String: Any] = ["port": Int(stream.port), "token": stream.token, "format": format]
        if format == "h264" { answer["transport"] = "annex-b" }
        return answer
    }

    func stopStream(on udid: String?, format: String?) async throws {
        let device = try find(udid).udid
        for key in streams.keys where key.hasPrefix(device + " ") && (format == nil || key == "\(device) \(format!)") {
            let stream = streams.removeValue(forKey: key)
            try? await stream?.operation?.stopStreaming()
            stream?.stop()
        }
    }

    func install(_ path: String, on udid: String?) async throws -> String {
        try await booted(udid).application.install(atPath: path).bundle.identifier
    }

    func launch(_ bundleId: String, arguments: [String], environment: [String: String], on udid: String?) async throws -> Int {
        let configuration = ApplicationLaunchConfiguration(
            bundleID: bundleId, bundleName: nil, arguments: arguments, environment: environment,
            waitForDebugger: false, io: FBProcessIO<AnyObject, AnyObject, AnyObject>.outputToDevNull(), launchMode: .relaunchIfRunning)
        return Int(try await booted(udid).application.launch(configuration).processIdentifier)
    }

    func terminate(_ bundleId: String, on udid: String?) async throws {
        try await booted(udid).application.kill(bundleID: bundleId)
    }

    func open(_ url: URL, on udid: String?) async throws {
        try await booted(udid).lifecycle.open(url)
    }

    private func set() throws -> SimulatorSet {
        if let control { return control.set }
        guard FileManager.default.fileExists(atPath: "/Library/Developer/PrivateFrameworks/CoreSimulator.framework") else {
            throw Failure(reason: "noXcode", message: "The iOS Simulator needs Xcode. Install it from the App Store and open it once.")
        }
        let started = try SimulatorControlBootstrap.withConfiguration(SimulatorControlConfiguration(deviceSetPath: nil, logger: nil))
        control = started
        return started.set
    }

    private func find(_ udid: String?) throws -> Simulator {
        let simulators = try set().allSimulators
        if simulators.isEmpty {
            throw Failure(reason: "noRuntime", message: "No iOS simulators. Add an iOS runtime in Xcode > Settings > Components.")
        }
        if let udid {
            guard let simulator = simulators.first(where: { $0.udid == udid }) else {
                throw Failure(reason: "notFound", message: "No simulator with id \(udid)")
            }
            return simulator
        }
        guard let simulator = simulators.first(where: { $0.state == .booted }) else {
            throw Failure(reason: "notBooted", message: "No simulator is running. Boot one first.")
        }
        return simulator
    }

    private func booted(_ udid: String?) async throws -> Simulator {
        let simulator = try find(udid)
        guard simulator.state == .booted else {
            throw Failure(reason: "notBooted", message: "\(simulator.name) is not running. Boot it first.")
        }
        return simulator
    }
}
