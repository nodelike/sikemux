import FBControlCore
import Foundation
import Network

/// A device's screen as length-prefixed frames on a socket on 127.0.0.1, which the app reads and
/// passes on to the page. A reader proves it is the app by sending the stream's token as its first line.
final class FrameStream: NSObject, DataConsumer, @unchecked Sendable {
    let token: String
    private let listener: NWListener
    private let queue = DispatchQueue(label: "sikemux-sim.frames")
    private var viewers: [NWConnection] = []
    var operation: (any VideoStreamOperation)?

    var port: UInt16 { listener.port?.rawValue ?? 0 }

    init(token: String = "sikemux-sim.\(UUID().uuidString.lowercased())") throws {
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
            var resumed = false
            listener.stateUpdateHandler = { state in
                guard !resumed else { return }
                switch state {
                case .ready:
                    resumed = true
                    continuation.resume()
                case let .failed(error):
                    resumed = true
                    continuation.resume(throwing: Failure(reason: "stream", message: "Could not open the screen stream: \(error)"))
                default:
                    break
                }
            }
            listener.start(queue: queue)
        }
    }

    func stop() {
        listener.cancel()
        queue.async { self.viewers.forEach { $0.cancel() } }
    }

    private func admit(_ connection: NWConnection) {
        connection.stateUpdateHandler = { [weak self, weak connection] state in
            guard let self, let connection else { return }
            switch state {
            case .ready:
                self.checkToken(of: connection)
            case .failed, .cancelled:
                self.viewers.removeAll { $0 === connection }
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
            if data == expected { self.viewers.append(connection) } else { connection.cancel() }
        }
    }

    func consumeData(_ data: Data) {
        queue.async {
            var length = UInt32(data.count).bigEndian
            let frame = Data(bytes: &length, count: 4) + data
            for viewer in self.viewers { viewer.send(content: frame, completion: .idempotent) }
        }
    }

    func consumeEndOfFile() {
        stop()
    }
}
