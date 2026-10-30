import FBControlCore
import Foundation
import Network
import os

/// A device's screen as length-prefixed frames on a socket on 127.0.0.1, which the app reads and
/// passes on to the page. A reader proves it is the app by sending the stream's token as its first line.
final class FrameStream: NSObject, DataConsumer, DataConsumerAsync, @unchecked Sendable {
    /// Frames a viewer may have unread before newer ones are dropped for it.
    static let inFlightLimit = 2

    let token: String
    /// H.264 frames build on the ones before, so a viewer that missed one waits for the next key frame.
    let framesDependOnEachOther: Bool
    private let listener: NWListener
    private let queue = DispatchQueue(label: "sikemux-sim.frames")
    private var viewers: [Viewer] = []
    private var quietSince = Date()
    private var keyFrameAsked = false
    private var stopped = false
    var operation: (any VideoStreamOperation)?
    /// Asks the encoder for a key frame now; also what shows a new viewer a screen that is not changing.
    var requestKeyFrame: @Sendable () -> Void = {}
    /// Called on the stream's queue when the last viewer leaves, and when the stream ends.
    var onQuiet: @Sendable () -> Void = {}

    var port: UInt16 { listener.port?.rawValue ?? 0 }

    init(framesDependOnEachOther: Bool, token: String = "sikemux-sim.\(UUID().uuidString.lowercased())") throws {
        self.framesDependOnEachOther = framesDependOnEachOther
        self.token = token
        let parameters = NWParameters.tcp
        parameters.requiredLocalEndpoint = NWEndpoint.hostPort(host: .ipv4(.loopback), port: .any)
        listener = try NWListener(using: parameters)
        super.init()
        listener.newConnectionHandler = { [weak self] connection in self?.admit(connection) }
    }

    /// Starts listening and returns once the port is known.
    func listen() async throws {
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            let settled = OSAllocatedUnfairLock(initialState: false)
            listener.stateUpdateHandler = { state in
                let outcome: Result<Void, Error>
                switch state {
                case .ready:
                    outcome = .success(())
                case let .failed(error):
                    outcome = .failure(Failure(reason: "stream", message: "Could not open the screen stream: \(error)"))
                default:
                    return
                }
                let first = settled.withLock { done in
                    defer { done = true }
                    return !done
                }
                if first { continuation.resume(with: outcome) }
            }
            listener.start(queue: queue)
        }
    }

    func stop() {
        listener.cancel()
        queue.async {
            self.stopped = true
            self.viewers.forEach { $0.connection.cancel() }
        }
    }

    /// Whether nobody has watched for at least `grace` seconds.
    func unwatched(for grace: TimeInterval) -> Bool {
        queue.sync { stopped || (viewers.isEmpty && Date().timeIntervalSince(quietSince) >= grace) }
    }

    /// Restarts the quiet period, for a stream just handed to a viewer that has not connected yet.
    func expectViewer() {
        queue.sync { quietSince = Date() }
    }

    private func admit(_ connection: NWConnection) {
        connection.stateUpdateHandler = { [weak self, weak connection] state in
            guard let self, let connection else { return }
            switch state {
            case .ready:
                self.checkToken(of: connection)
            case .failed, .cancelled:
                self.leave(connection)
            default:
                break
            }
        }
        connection.start(queue: queue)
    }

    private func checkToken(of connection: NWConnection) {
        let expected = Data((token + "\n").utf8)
        connection.receive(minimumIncompleteLength: expected.count, maximumLength: expected.count) { [weak self] data, _, _, _ in
            guard let self else { return }
            guard data == expected, !self.stopped else { return connection.cancel() }
            self.viewers.append(Viewer(connection))
            self.watchForClose(connection)
            self.askForKeyFrame()
        }
    }

    /// A viewer sends nothing after its token, so anything more, or the end, means it has gone.
    private func watchForClose(_ connection: NWConnection) {
        connection.receive(minimumIncompleteLength: 1, maximumLength: 64) { [weak self] _, _, isComplete, error in
            if isComplete || error != nil { connection.cancel() } else { self?.watchForClose(connection) }
        }
    }

    private func leave(_ connection: NWConnection) {
        let before = viewers.count
        viewers.removeAll { $0.connection === connection }
        if before > 0 && viewers.isEmpty {
            quietSince = Date()
            onQuiet()
        }
    }

    private func askForKeyFrame() {
        guard !keyFrameAsked else { return }
        keyFrameAsked = true
        requestKeyFrame()
    }

    /// The encoder skips a frame while this is above two; it is, when nobody is watching or every viewer
    /// is behind, and each of them then needs a fresh frame once it catches up.
    func unprocessedDataCount() -> Int {
        queue.sync {
            guard viewers.contains(where: { $0.inFlight < Self.inFlightLimit }) else {
                viewers.forEach { $0.missedFrame = true }
                keyFrameAsked = false
                return Int.max
            }
            return 0
        }
    }

    func consumeData(_ data: Data) {
        queue.async { [self] in
            let key = !self.framesDependOnEachOther || Self.isKeyFrame(data)
            if key { self.keyFrameAsked = false }
            var length = UInt32(data.count).bigEndian
            let header = Data(bytes: &length, count: 4)
            for viewer in self.viewers {
                guard viewer.inFlight < Self.inFlightLimit, key || !viewer.missedFrame else {
                    viewer.missedFrame = true
                    continue
                }
                viewer.missedFrame = false
                viewer.inFlight += 1
                viewer.connection.batch {
                    viewer.connection.send(content: header, completion: .idempotent)
                    viewer.connection.send(content: data, completion: .contentProcessed { [weak self, weak viewer] _ in
                        guard let self, let viewer else { return }
                        viewer.inFlight -= 1
                        if viewer.missedFrame && viewer.inFlight < Self.inFlightLimit { self.askForKeyFrame() }
                    })
                }
            }
        }
    }

    /// An Annex-B frame starts with a 4-byte start code; a key frame's first unit is its parameter set
    /// (type 7), or the picture itself (type 5).
    static func isKeyFrame(_ frame: Data) -> Bool {
        guard frame.count > 4 else { return false }
        let type = frame[frame.startIndex + 4] & 0x1F
        return type == 7 || type == 5
    }

    func consumeEndOfFile() {
        stop()
        queue.async { self.onQuiet() }
    }
}

private final class Viewer {
    let connection: NWConnection
    var inFlight = 0
    var missedFrame = true

    init(_ connection: NWConnection) {
        self.connection = connection
    }
}
