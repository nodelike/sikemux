import CoreGraphics
import CoreSimulator
import FBControlCore
import FBSimulatorControl
import Foundation
import ImageIO

/// The device work. One per helper process, so CoreSimulator is loaded once and HID connections are reused.
actor Simulators {
    private var control: SimulatorControlBootstrap?
    /// Each table holds a task from the moment it starts, so a second request arriving while the
    /// first is still connecting waits for it instead of starting another.
    private var touch: [String: Task<SimulatorHID, Error>] = [:]
    private var tails: [String: Task<LogTail, Error>] = [:]
    private var boots: [String: Task<Void, Error>] = [:]
    private var streams: [String: Task<FrameStream, Error>] = [:]
    /// A finger the person is holding on the screen, fed move by move into one gesture so each move
    /// costs a send rather than a whole tap's settling.
    private var fingers: [String: Finger] = [:]
    private var known: [String: Simulator] = [:]
    private var chromes: [String: [String: Any]] = [:]
    /// The orientation each device was last turned to here, for runtimes that cannot report it.
    private var turned: [String: String] = [:]
    /// Devices whose runtime cannot report its orientation, and so turns by the older event that
    /// leaves touches in the screen's own portrait points.
    private var portraitTouches: Set<String> = []

    func devices() throws -> [[String: Any]] {
        try iPhones().map { simulator in
            if simulator.state != .booted { forget(simulator.udid) }
            return [
                "udid": simulator.udid,
                "name": simulator.name,
                "state": simulator.state == .booted ? "booted" : simulator.state == .shutdown ? "shutdown" : "busy",
                "runtime": simulator.osVersion.name.rawValue,
                "model": simulator.deviceType.model.rawValue,
            ]
        }
    }

    /// The device's screen in points as it is turned now, which is what touches are given in, which way it
    /// is turned, and its pixels per point. Screenshots and streamed frames stay upright whichever way it is.
    func screen(_ udid: String?) async throws -> [String: Any] {
        guard let info = try await booted(udid).screenInfo, info.scale > 0 else {
            throw Failure(reason: "simulator", message: "The device did not report its screen size")
        }
        let scale = Double(info.scale)
        let orientation = try await orientation(udid)
        let size = Self.turnedSize(CGSize(width: Double(info.widthPixels) / scale, height: Double(info.heightPixels) / scale), orientation)
        return ["width": size.width, "height": size.height, "scale": scale, "orientation": orientation]
    }

    /// The device's screen outline at full resolution, upright, as a PNG data URL: what Simulator clips
    /// its picture to, rounded corners and all. Nil for a device type that has none.
    func mask(_ udid: String?) async throws -> String? {
        let simulator = try await booted(udid)
        guard let bundle = simulator.device.deviceType?.bundle,
            let profile = bundle.url(forResource: "profile", withExtension: "plist").flatMap({ NSDictionary(contentsOf: $0) }),
            let name = profile["framebufferMask"] as? String,
            let url = bundle.url(forResource: name, withExtension: "pdf")
        else { return nil }
        return Self.png(url, scale: 1)?.url
    }

    /// The device's frame as Simulator draws it from Xcode's DeviceKit chrome: the bezel with its screen
    /// opening, and each side button with where it sits, how far it slides out under the pointer and the
    /// image it shows pressed. Sizes are in points; images are PNG data URLs at the screen's scale.
    func chrome(_ udid: String?) async throws -> [String: Any]? {
        let simulator = try await booted(udid)
        guard let bundle = simulator.device.deviceType?.bundle,
            let profile = bundle.url(forResource: "profile", withExtension: "plist").flatMap({ NSDictionary(contentsOf: $0) }),
            let identifier = profile["chromeIdentifier"] as? String,
            let info = simulator.screenInfo, info.scale > 0
        else { return nil }
        let scale = Double(info.scale)
        let key = "\(identifier) \(scale)"
        if let known = chromes[key] { return known }
        let name = identifier.split(separator: ".").last.map(String.init) ?? identifier
        let resources = URL(fileURLWithPath: "/Library/Developer/DeviceKit/Chrome/\(name).devicechrome/Contents/Resources")
        guard let data = try? Data(contentsOf: resources.appendingPathComponent("chrome.json")),
            let layout = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
            let images = layout["images"] as? [String: Any],
            let compositeName = images["composite"] as? String,
            let composite = Self.png(resources.appendingPathComponent("\(compositeName).pdf"), scale: scale)
        else { return nil }
        let padding = images["devicePadding"] as? [String: Double] ?? [:]
        let buttons: [[String: Any]] = (layout["inputs"] as? [[String: Any]] ?? []).compactMap { input in
            guard input["type"] as? String == "button",
                let image = (input["image"] as? String).flatMap({ Self.png(resources.appendingPathComponent("\($0).pdf"), scale: scale) }),
                let offsets = input["offsets"] as? [String: [String: Double]],
                let normal = offsets["normal"]
            else { return nil }
            let down = (input["imageDown"] as? String).flatMap { Self.png(resources.appendingPathComponent("\($0).pdf"), scale: scale) }
            return [
                "name": input["name"] as? String ?? "",
                "title": input["accessibilityTitle"] as? String ?? "",
                "anchor": input["anchor"] as? String ?? "left",
                "x": normal["x"] ?? 0,
                "y": normal["y"] ?? 0,
                "hoverX": offsets["rollover"]?["x"] ?? normal["x"] ?? 0,
                "width": image.width / scale,
                "height": image.height / scale,
                "image": image.url,
                "imageDown": down?.url ?? NSNull(),
            ]
        }
        let answer: [String: Any] = [
            "image": composite.url,
            "width": composite.width / scale,
            "height": composite.height / scale,
            "padding": ["top": padding["top"] ?? 0, "left": padding["left"] ?? 0, "bottom": padding["bottom"] ?? 0, "right": padding["right"] ?? 0],
            "buttons": buttons,
        ]
        chromes[key] = answer
        return answer
    }

    /// The first page of a PDF drawn `scale` pixels to its point, as a PNG data URL, with its size in pixels.
    static func png(_ url: URL, scale: Double) -> (url: String, width: Double, height: Double)? {
        guard let page = CGPDFDocument(url as CFURL)?.page(at: 1) else { return nil }
        let box = page.getBoxRect(.mediaBox)
        let width = Int((box.width * scale).rounded())
        let height = Int((box.height * scale).rounded())
        guard width > 0, height > 0,
            let context = CGContext(
                data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
                space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
        else { return nil }
        context.scaleBy(x: scale, y: scale)
        context.drawPDFPage(page)
        guard let image = context.makeImage() else { return nil }
        let png = NSMutableData()
        guard let destination = CGImageDestinationCreateWithData(png, "public.png" as CFString, 1, nil) else { return nil }
        CGImageDestinationAddImage(destination, image, nil)
        guard CGImageDestinationFinalize(destination) else { return nil }
        return ("data:image/png;base64," + (png as Data).base64EncodedString(), Double(width), Double(height))
    }

    static func turnedSize(_ upright: CGSize, _ orientation: String) -> CGSize {
        orientation.hasPrefix("landscape") ? CGSize(width: upright.height, height: upright.width) : upright
    }

    func orient(_ udid: String?, to name: String) async throws {
        let settable: [SimulatorDeviceOrientation] = [.portrait, .portraitUpsideDown, .landscapeLeft, .landscapeRight]
        guard let orientation = SimulatorDeviceOrientation(rawValue: name), settable.contains(orientation) else {
            throw Failure(
                reason: "badRequest", message: "Unknown orientation \(name). Use portrait, portraitUpsideDown, landscapeLeft or landscapeRight.")
        }
        let simulator = try await booted(udid)
        try await simulator.orientation.set(orientation)
        turned[simulator.udid] = name
    }

    /// Older runtimes cannot report the orientation, so the last one set here stands in, else portrait.
    func orientation(_ udid: String?) async throws -> String {
        let simulator = try await booted(udid)
        do {
            return try await simulator.orientation.current().rawValue
        } catch where unsupportedSimulatorCapability(in: error) != nil {
            portraitTouches.insert(simulator.udid)
        } catch {}
        return turned[simulator.udid] ?? "portrait"
    }

    /// Where to touch a point given in the turned screen's points. Runtimes that report their
    /// orientation take those as they are; on the others the screen keeps its portrait points, and a
    /// turned app is drawn sideways in them.
    func touchPoint(_ point: CGPoint, on udid: String?) async throws -> CGPoint {
        let simulator = try await booted(udid)
        guard portraitTouches.contains(simulator.udid), let info = simulator.screenInfo, info.scale > 0 else { return point }
        let scale = Double(info.scale)
        let portrait = CGSize(width: Double(info.widthPixels) / scale, height: Double(info.heightPixels) / scale)
        return Self.portraitPoint(point, turned: turned[simulator.udid], screen: portrait)
    }

    /// Where a point on a turned screen is drawn on its portrait screen, `screen` in points.
    static func portraitPoint(_ point: CGPoint, turned: String?, screen: CGSize) -> CGPoint {
        switch turned {
        case "landscapeLeft": CGPoint(x: screen.width - point.y, y: point.x)
        case "landscapeRight": CGPoint(x: point.y, y: screen.height - point.x)
        case "portraitUpsideDown": CGPoint(x: screen.width - point.x, y: screen.height - point.y)
        default: point
        }
    }

    func touchPoints(_ points: [TouchPoint], on udid: String?) async throws -> [TouchPoint] {
        var placed: [TouchPoint] = []
        for point in points {
            let first = try await touchPoint(CGPoint(x: point.x, y: point.y), on: udid)
            var second: CGPoint?
            if let x2 = point.x2, let y2 = point.y2 { second = try await touchPoint(CGPoint(x: x2, y: y2), on: udid) }
            placed.append(TouchPoint(x: first.x, y: first.y, x2: second.map { Double($0.x) }, y2: second.map { Double($0.y) }, t: point.t))
        }
        return placed
    }

    /// A swipe starting at a side of the screen is tagged with it, so iOS treats it as the system
    /// gesture a finger there would make: home, back, Notification Center or Control Center.
    func edge(at point: CGPoint, on udid: String?) async throws -> SimulatorHIDEdge {
        let simulator = try await booted(udid)
        guard let info = simulator.screenInfo, info.scale > 0 else { return .none }
        let scale = Double(info.scale)
        let sideways = try await orientation(udid).hasPrefix("landscape")
        let (short, long) = (Double(info.widthPixels) / scale, Double(info.heightPixels) / scale)
        let (width, height) = sideways ? (long, short) : (short, long)
        let reach = 10.0
        if point.y >= height - reach { return .bottom }
        if point.y <= reach { return .top }
        if point.x <= reach { return .left }
        if point.x >= width - reach { return .right }
        return .none
    }

    func runtimes() throws -> [[String: Any]] {
        _ = try set()
        return (control?.serviceContext.supportedRuntimes() ?? []).map { runtime in
            ["identifier": runtime.identifier ?? "", "name": runtime.name ?? "", "version": runtime.versionString ?? "", "available": runtime.available]
        }
    }

    /// Boots a device, or waits for the boot already under way, until it can be used.
    func boot(_ udid: String?) async throws {
        let simulator = try find(udid)
        if simulator.state != .booted { forget(simulator.udid) }
        let booting = boots[simulator.udid] ?? Task { try await Self.bootUntilUsable(simulator) }
        boots[simulator.udid] = booting
        defer { if boots[simulator.udid] == booting { boots[simulator.udid] = nil } }
        try await booting.value
    }

    static let bootLimit: TimeInterval = 150

    private static func bootUntilUsable(_ simulator: Simulator) async throws {
        let started = Date()
        func deadline() -> PollDeadline {
            PollDeadline(timeout: max(bootLimit - Date().timeIntervalSince(started), 1), waitingFor: "\(simulator.name) to boot")
        }
        do {
            if simulator.state == .shuttingDown { try await TargetResolveLeavesState(simulator, .shuttingDown, deadline: deadline()) }
            if simulator.state == .shutdown { try await simulator.lifecycle.boot(SimulatorBootConfiguration(options: [], environment: [:])) }
            try await simulator.lifecycle.resolveUsable(deadline: deadline())
        } catch is PollTimeoutError {
            throw Failure(
                reason: "bootTimeout",
                message: "\(simulator.name) did not finish booting in \(Int(bootLimit)) seconds. Shut it down and boot it again.")
        }
    }

    func shutdown(_ udid: String?) async throws {
        let simulator = try find(udid)
        forget(simulator.udid)
        try await set().shutdown(simulator)
    }

    /// Drops what is held for a device that is not running, whoever shut it down, so its next use starts afresh.
    private func forget(_ udid: String) {
        touch[udid] = nil
        known[udid] = nil
        fingers.removeValue(forKey: udid)?.events.finish()
        turned[udid] = nil
        portraitTouches.remove(udid)
        for key in tails.keys where key.hasPrefix(udid + " ") {
            let following = tails.removeValue(forKey: key)
            Task { try? await following?.value.stop() }
        }
        for key in streams.keys where key.hasPrefix(udid + " ") {
            let starting = streams.removeValue(forKey: key)
            Task { await Self.stop(starting) }
        }
    }

    /// `pointSize` draws one pixel per point, a ninth of a Retina screenshot's pixels, for agents to read.
    func screenshot(_ udid: String?, jpeg: Bool = false, pointSize: Bool = false) async throws -> (data: Data, width: Int, height: Int) {
        let simulator = try await booted(udid)
        let scale = pointSize ? Double(simulator.screenInfo?.scale ?? 1) : 1
        let configuration = ScreenshotConfiguration(
            encoding: jpeg ? .jpeg(quality: 0.8) : .png, scale: scale > 1 ? .factor(1 / scale) : .native)
        let shot = try await simulator.screenshot.take(configuration: configuration)
        return (shot.imageData, Int(shot.size.width), Int(shot.size.height))
    }

    func tree(_ udid: String?) async throws -> Any {
        let response = try await booted(udid).uiAutomation(backend: .accessibility)
            .describe(.frontmost, options: AccessibilityRequestOptions(format: .nested, enableLogging: false))
        return try JSONSerialization.jsonObject(with: JSONEncoder().encode(response.elements.elements))
    }

    /// Looks again every quarter second until `wait` runs out, since a label is often still on its way in
    /// just after a tap, a key press or a screen change, and a screen just booted has no accessibility yet.
    func frame(of label: String, on udid: String?, wait: TimeInterval) async throws -> CGRect {
        let automation = try await booted(udid).uiAutomation(backend: .accessibility)
        let deadline = Date().addingTimeInterval(wait)
        while true {
            do {
                return try await automation.frame(.marker(value: label, key: .label, depth: .max))
            } catch UIAutomationError.elementNotFound, UIAutomationError.elementNotOnScreen, AccessibilityError.noTranslationObject {
                guard Date() < deadline else { throw Failure(reason: "notFound", message: "Nothing labelled \"\(label)\" is on screen") }
                try await Task.sleep(nanoseconds: 250_000_000)
            }
        }
    }

    /// A connection that fails is dropped and made again once, since the device may have restarted
    /// since it was made.
    func send(_ event: SimulatorHIDEvent, to udid: String?) async throws {
        let simulator = try await booted(udid)
        let connecting = connection(to: simulator)
        do {
            try await connecting.value.send(event: event, logger: simulator.logger)
        } catch where !(error is CancellationError) {
            if touch[simulator.udid] == connecting { touch[simulator.udid] = nil }
            try await connection(to: simulator).value.send(event: event, logger: simulator.logger)
        }
    }

    private func connection(to simulator: Simulator) -> Task<SimulatorHID, Error> {
        if let connecting = touch[simulator.udid] { return connecting }
        let connecting = Task { try await simulator.hid.connect() }
        touch[simulator.udid] = connecting
        return connecting
    }

    /// A live touch from the person: `down` starts a gesture, `move`s join it as they come, and `up`
    /// ends it and waits until the device has taken it all in.
    func touch(_ phase: String, at turnedPoint: CGPoint, on udid: String?) async throws {
        let simulator = try await booted(udid)
        let point = try await touchPoint(turnedPoint, on: simulator.udid)
        switch phase {
        case "down":
            await lift(simulator.udid, at: nil)
            let finger = try await press(simulator, edge: try await edge(at: turnedPoint, on: simulator.udid))
            fingers[simulator.udid] = finger
            finger.put(.down, point)
        case "move":
            guard let finger = fingers[simulator.udid] else { return try await touch("down", at: turnedPoint, on: udid) }
            finger.put(.down, point)
        case "up":
            await lift(simulator.udid, at: point)
        default:
            throw Failure(reason: "badRequest", message: "A touch's phase is down, move or up")
        }
    }

    private func press(_ simulator: Simulator, edge: SimulatorHIDEdge) async throws -> Finger {
        let hid = try await connection(to: simulator).value
        let (events, feed) = AsyncStream<SimulatorHIDEvent>.makeStream()
        let gesture = Task { try await hid.send(events: events, logger: simulator.logger) }
        return Finger(events: feed, gesture: gesture, edge: edge)
    }

    private func lift(_ udid: String, at point: CGPoint?) async {
        guard let finger = fingers.removeValue(forKey: udid) else { return }
        if let point { finger.put(.up, point) }
        finger.events.finish()
        if (try? await finger.gesture.value) == nil, touch[udid] != nil { touch[udid] = nil }
    }

    /// Starts following a device's log the first time it is asked for, filtered to one process if one is named.
    /// Once the whole device is followed, a process's lines come from there, kept since it started.
    func logs(on udid: String?, process: String?, after cursor: Int, generation: Int?, limit: Int) async throws -> [String: Any] {
        let simulator = try await booted(udid)
        if let process, tails["\(simulator.udid) "] != nil {
            return try await follow(simulator, process: nil).read(process: process, after: cursor, limit: limit, generation: generation)
        }
        return try await follow(simulator, process: process).read(after: cursor, limit: limit, generation: generation)
    }

    private func follow(_ simulator: Simulator, process: String?) async throws -> LogTail {
        let key = "\(simulator.udid) \(process ?? "")"
        let following = tails[key] ?? Task {
            let tail = LogTail()
            var arguments = ["--style", "compact"]
            if let process {
                arguments += ["--predicate", "process == \"\(process.replacingOccurrences(of: "\"", with: ""))\""]
            } else {
                // Apple's frameworks log thousands of activity and debug entries a second, many with no
                // subsystem to exclude them by; ordinary log messages at Info and above keep an app's own.
                arguments += ["--type", "log", "--level", "info", "--predicate", "NOT (subsystem BEGINSWITH \"com.apple.\")"]
            }
            tail.attach(try await simulator.log.tail(arguments: arguments, consumer: tail.consumer))
            return tail
        }
        tails[key] = following
        do {
            return try await following.value
        } catch {
            if tails[key] == following { tails[key] = nil }
            throw error
        }
    }

    func stopLogs(on udid: String?, process: String?) throws {
        let following = tails.removeValue(forKey: "\(try find(udid).udid) \(process ?? "")")
        Task { try? await following?.value.stop() }
    }

    /// One stream per device and format, shared by every viewer and stopped once the last has been gone
    /// for `grace` seconds, so hiding and showing the screen again reuses it. Frames are encoded only when
    /// the screen changes, at most 60 a second. Each new viewer is sent a key frame at once, and H.264
    /// sends another every four seconds.
    func stream(on udid: String?, format: String, scale: Double?) async throws -> [String: Any] {
        let simulator = try await booted(udid)
        _ = connection(to: simulator)
        let videoFormat: VideoStreamFormat
        switch format {
        case "h264": videoFormat = .compressedVideo(withCodec: .h264, transport: .annexB)
        case "mjpeg": videoFormat = .mjpeg(encoder: .allowSoftware)
        default: throw Failure(reason: "badRequest", message: "Unknown stream format \(format). Use h264 or mjpeg.")
        }
        let key = "\(simulator.udid) \(format)"
        let starting = streams[key] ?? Task { [self] in
            let configuration = VideoStreamConfiguration(
                format: videoFormat, framesPerSecond: nil, rateControl: nil, scaleFactor: scale, keyFrameRate: 4)
            return try await Self.start(configuration, on: simulator, format: format) {
                Task { await self.stopWhenUnwatched(key) }
            }
        }
        streams[key] = starting
        let stream: FrameStream
        do {
            stream = try await starting.value
        } catch {
            if streams[key] == starting { streams[key] = nil }
            throw error
        }
        stream.expectViewer()
        Task { await stopWhenUnwatched(key) }
        var answer: [String: Any] = ["port": Int(stream.port), "token": stream.token, "format": format]
        if format == "h264" { answer["transport"] = "annex-b" }
        return answer
    }

    static let grace: TimeInterval = 2

    private static func start(
        _ configuration: VideoStreamConfiguration, on simulator: Simulator, format: String, onQuiet: @escaping @Sendable () -> Void
    ) async throws -> FrameStream {
        let stream = try FrameStream(framesDependOnEachOther: format == "h264")
        stream.onQuiet = onQuiet
        do {
            try await stream.listen()
            let operation = try await simulator.videoStream.create(configuration: configuration, to: stream)
            stream.operation = operation
            if let video = operation as? SimulatorVideoStream { stream.requestKeyFrame = { video.requestKeyFrame() } }
            stream.requestKeyFrame()
            return stream
        } catch {
            stream.stop()
            let name = format == "h264" ? "H.264" : "MJPEG"
            throw Failure(reason: "streamUnavailable", message: "Could not start the \(name) screen stream: \(Failure.describe(error))")
        }
    }

    private func stopWhenUnwatched(_ key: String) async {
        guard let starting = streams[key] else { return }
        try? await Task.sleep(nanoseconds: UInt64(Self.grace * 1_000_000_000))
        guard streams[key] == starting, let stream = try? await starting.value, stream.unwatched(for: Self.grace) else { return }
        streams[key] = nil
        await Self.stop(starting)
    }

    /// Stops a device's streams at once, whoever is watching.
    func stopStream(on udid: String?, format: String?) async throws {
        let device = try find(udid).udid
        for key in streams.keys where key.hasPrefix(device + " ") && (format == nil || key == "\(device) \(format!)") {
            await Self.stop(streams.removeValue(forKey: key))
        }
    }

    private static func stop(_ starting: Task<FrameStream, Error>?) async {
        guard let stream = try? await starting?.value else { return }
        try? await stream.operation?.stopStreaming()
        stream.stop()
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
            throw Failure(reason: "noXcode", message: Xcode.missing)
        }
        try Xcode.choose()
        let started = try SimulatorControlBootstrap.withConfiguration(SimulatorControlConfiguration(deviceSetPath: nil, logger: nil))
        control = started
        return started.set
    }

    /// The iOS devices whose runtime is installed; watchOS, tvOS and visionOS ones, and any left
    /// behind by a deleted runtime, cannot be driven here.
    private func iPhones() throws -> [Simulator] {
        try set().allSimulators.filter { simulator in
            let runtime: SimRuntime? = simulator.device.runtime
            return simulator.device.available && runtime?.available == true && runtime?.platformIdentifier == "com.apple.platform.iphonesimulator"
        }
    }

    private func find(_ udid: String?) throws -> Simulator {
        let simulators = try iPhones()
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
        if let udid, let simulator = known[udid], simulator.state == .booted { return simulator }
        let simulator = try find(udid)
        guard simulator.state == .booted else {
            forget(simulator.udid)
            throw Failure(reason: "notBooted", message: "\(simulator.name) is not running. Boot it first.")
        }
        known[simulator.udid] = simulator
        return simulator
    }
}

private struct Finger {
    let events: AsyncStream<SimulatorHIDEvent>.Continuation
    let gesture: Task<Void, Error>
    let edge: SimulatorHIDEdge

    func put(_ direction: SimulatorHIDDirection, _ point: CGPoint) {
        events.yield(.touch(direction: direction, x: point.x, y: point.y, edge: edge))
    }
}
