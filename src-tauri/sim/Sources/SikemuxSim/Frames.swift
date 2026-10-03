import FBControlCore
import Foundation
import Network

/// A device's screen as H.264, served to the app over a WebSocket on 127.0.0.1. A viewer
/// proves it is the app by offering the stream's token as its WebSocket subprotocol.
final class FrameStream: NSObject, DataConsumer, @unchecked Sendable {
    let token: String
    private let listener: NWListener
    private let queue = DispatchQueue(label: "sikemux-sim.frames")
    private var viewers: [NWConnection] = []
    var operation: (any VideoStreamOperation)?

    var port: UInt16 { listener.port?.rawValue ?? 0 }

    init(token: String = "sikemux-sim.\(UUID().uuidString.lowercased())") throws {
        self.token = token
        let socket = NWProtocolWebSocket.Options()
        socket.setClientRequestHandler(queue) { subprotocols, _ in
            subprotocols.contains(token) ? .init(status: .accept, subprotocol: token) : .init(status: .reject, subprotocol: nil)
        }
        let parameters = NWParameters.tcp
        parameters.requiredLocalEndpoint = NWEndpoint.hostPort(host: .ipv4(.loopback), port: .any)
        parameters.defaultProtocolStack.applicationProtocols.insert(socket, at: 0)
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
                self.viewers.append(connection)
            case .failed, .cancelled:
                self.viewers.removeAll { $0 === connection }
            default:
                break
            }
        }
        connection.start(queue: queue)
    }

    func consumeData(_ data: Data) {
        queue.async {
            let metadata = NWProtocolWebSocket.Metadata(opcode: .binary)
            let context = NWConnection.ContentContext(identifier: "frame", metadata: [metadata])
            for viewer in self.viewers {
                viewer.send(content: data, contentContext: context, isComplete: true, completion: .idempotent)
            }
        }
    }

    func consumeEndOfFile() {
        stop()
    }
}
