import Foundation

enum CoreLinkError: Error {
    case closed
    case refused(String)

    var message: String {
        switch self {
        case .closed: return "Sikemux's core went away"
        case .refused(let message): return message
        }
    }
}

/// One connection to the core, speaking its frames: a 4-byte big-endian length,
/// a kind byte, then JSON for control frames. Callbacks arrive on the main queue.
final class CoreLink {
    typealias Reply = (Result<[String: Any], CoreLinkError>) -> Void

    /// The kind byte and the core's largest payload, `MAX_FRAME_BYTES` in sikemux-wire.
    private static let maxFrameLength = 1 + 16 * 1024 * 1024

    var onReady: (() -> Void)?
    var onEvent: (([String: Any]) -> Void)?
    var onClose: ((String?) -> Void)?

    private let socketPath: String
    private let protocolVersion: Int
    private let queue = DispatchQueue(label: "com.nodelike.sikemux.notch.core")
    private var fd: Int32 = -1
    private var source: DispatchSourceRead?
    private var buffer: [UInt8] = []
    private var nextRequest: UInt64 = 1
    private var replies: [UInt64: Reply] = [:]
    private var closed = false

    init(socket: String, protocolVersion: Int) {
        socketPath = socket
        self.protocolVersion = protocolVersion
    }

    /// False when nothing listens on the socket.
    func connect() -> Bool {
        guard var address = UnixAddress.of(socketPath) else { return false }
        let descriptor = socket(AF_UNIX, SOCK_STREAM, 0)
        guard descriptor >= 0 else { return false }
        let connected = withUnsafePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                Darwin.connect(descriptor, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard connected == 0 else {
            Darwin.close(descriptor)
            return false
        }
        var on: Int32 = 1
        setsockopt(descriptor, SOL_SOCKET, SO_NOSIGPIPE, &on, socklen_t(MemoryLayout<Int32>.size))
        fd = descriptor
        let source = DispatchSource.makeReadSource(fileDescriptor: descriptor, queue: queue)
        source.setEventHandler { [weak self] in self?.readAvailable() }
        source.resume()
        self.source = source
        queue.async {
            self.write(["type": "hello", "protocol": "sikemux-core", "version": self.protocolVersion])
        }
        return true
    }

    func request(_ request: [String: Any], reply: Reply? = nil) {
        queue.async {
            guard !self.closed else {
                DispatchQueue.main.async { reply?(.failure(.closed)) }
                return
            }
            let id = self.nextRequest
            self.nextRequest += 1
            if let reply { self.replies[id] = reply }
            self.write(["type": "request", "requestId": id, "request": request])
        }
    }

    func close() {
        queue.async { self.finish(nil) }
    }

    private func write(_ message: [String: Any]) {
        guard let json = try? JSONSerialization.data(withJSONObject: message) else { return }
        var frame = [UInt8]()
        let length = UInt32(json.count + 1).bigEndian
        withUnsafeBytes(of: length) { frame.append(contentsOf: $0) }
        frame.append(0)
        frame.append(contentsOf: json)
        var offset = 0
        while offset < frame.count {
            let written = frame[offset...].withUnsafeBytes { Darwin.write(fd, $0.baseAddress, $0.count) }
            if written <= 0 {
                finish("could not write to the core")
                return
            }
            offset += written
        }
    }

    private func readAvailable() {
        var chunk = [UInt8](repeating: 0, count: 64 * 1024)
        let count = chunk.withUnsafeMutableBytes { Darwin.read(fd, $0.baseAddress, $0.count) }
        guard count > 0 else {
            finish(nil)
            return
        }
        buffer.append(contentsOf: chunk[0..<count])
        var offset = 0
        while !closed, buffer.count - offset >= 4 {
            let length = buffer[offset..<(offset + 4)].reduce(0) { $0 << 8 | Int($1) }
            guard length > 0, length <= Self.maxFrameLength else {
                finish("the core sent a frame of \(length) bytes, which is out of range")
                return
            }
            let end = offset + 4 + length
            guard buffer.count >= end else { break }
            if buffer[offset + 4] == 0 { handle(Data(buffer[(offset + 5)..<end])) }
            offset = end
        }
        buffer.removeFirst(offset)
    }

    private func handle(_ payload: Data) {
        guard let message = (try? JSONSerialization.jsonObject(with: payload)) as? [String: Any],
              let type = message["type"] as? String
        else { return }
        switch type {
        case "helloAck":
            DispatchQueue.main.async { self.onReady?() }
        case "helloRejected":
            finish(message["message"] as? String ?? "the core speaks another protocol version")
        case "response":
            guard let id = (message["requestId"] as? NSNumber)?.uint64Value,
                  let reply = replies.removeValue(forKey: id)
            else { return }
            let response = message["response"] as? [String: Any] ?? [:]
            DispatchQueue.main.async { reply(.success(response)) }
        case "error":
            let text = message["message"] as? String ?? "the core refused"
            guard let id = (message["requestId"] as? NSNumber)?.uint64Value,
                  let reply = replies.removeValue(forKey: id)
            else { return }
            DispatchQueue.main.async { reply(.failure(.refused(text))) }
        case "event":
            guard let event = message["event"] as? [String: Any] else { return }
            DispatchQueue.main.async { self.onEvent?(event) }
        default:
            break
        }
    }

    private func finish(_ reason: String?) {
        guard !closed else { return }
        closed = true
        source?.cancel()
        source = nil
        if fd >= 0 {
            Darwin.close(fd)
            fd = -1
        }
        let pending = replies
        replies.removeAll()
        DispatchQueue.main.async {
            for reply in pending.values { reply(.failure(.closed)) }
            self.onClose?(reason)
        }
    }
}

enum UnixAddress {
    /// Nil for a path too long for a socket address.
    static func of(_ path: String) -> sockaddr_un? {
        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        let bytes = Array(path.utf8)
        guard bytes.count < MemoryLayout.size(ofValue: address.sun_path) else { return nil }
        withUnsafeMutableBytes(of: &address.sun_path) { raw in
            raw.copyBytes(from: bytes)
            raw[bytes.count] = 0
        }
        return address
    }
}

/// A socket in the folder beside the core's that the core writes to whenever
/// it starts, stops or opens a window, so the notch tries the core again then
/// rather than on a timer.
final class CoreWaiter {
    private let path: String
    private let descriptor: Int32
    private let source: DispatchSourceRead

    init?(socket: String, woken: @escaping () -> Void) {
        let folder = socket + ".waiting"
        mkdir(folder, 0o700)
        path = folder + "/notch-\(getpid())"
        unlink(path)
        guard var address = UnixAddress.of(path) else { return nil }
        let descriptor = Darwin.socket(AF_UNIX, SOCK_DGRAM, 0)
        guard descriptor >= 0 else { return nil }
        let bound = withUnsafePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                bind(descriptor, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard bound == 0 else {
            Darwin.close(descriptor)
            return nil
        }
        _ = fcntl(descriptor, F_SETFL, O_NONBLOCK)
        self.descriptor = descriptor
        source = DispatchSource.makeReadSource(fileDescriptor: descriptor, queue: .main)
        source.setEventHandler { [descriptor] in
            var signal: UInt8 = 0
            while recv(descriptor, &signal, 1, 0) > 0 {}
            woken()
        }
        source.resume()
    }

    deinit {
        source.cancel()
        Darwin.close(descriptor)
        unlink(path)
    }
}

/// Starts the core with the host's `start` command, which goes through the login
/// item when one is installed, so no second core runs beside it. `done` runs on the
/// main queue once the command ends: the core answers by then, or `start` gave up.
enum CoreStarter {
    static func start(_ options: Options, done: @escaping () -> Void) {
        guard let binary = options.coreBinary else { return done() }
        var environment = ProcessInfo.processInfo.environment
        environment["SIKEMUX_CORE_SOCKET"] = options.socket
        DispatchQueue.global(qos: .userInitiated).async {
            let argv: [UnsafeMutablePointer<CChar>?] = [strdup(binary), strdup("start"), nil]
            let envp: [UnsafeMutablePointer<CChar>?] = environment.map { strdup("\($0.key)=\($0.value)") } + [nil]
            defer { (argv + envp).forEach { free($0) } }
            var attributes: posix_spawnattr_t?
            posix_spawnattr_init(&attributes)
            defer { posix_spawnattr_destroy(&attributes) }
            posix_spawnattr_setflags(&attributes, Int16(POSIX_SPAWN_CLOEXEC_DEFAULT))
            var actions: posix_spawn_file_actions_t?
            posix_spawn_file_actions_init(&actions)
            defer { posix_spawn_file_actions_destroy(&actions) }
            posix_spawn_file_actions_addopen(&actions, 0, "/dev/null", O_RDONLY, 0)
            posix_spawn_file_actions_addinherit_np(&actions, 1)
            posix_spawn_file_actions_addinherit_np(&actions, 2)
            var pid: pid_t = 0
            if posix_spawn(&pid, binary, &actions, &attributes, argv, envp) == 0 { waitpid(pid, nil, 0) }
            DispatchQueue.main.async(execute: done)
        }
    }
}
