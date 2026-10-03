import FBControlCore
import Foundation

/// The latest lines of one device's log, numbered from 1 so a reader can ask for what came after the last line it saw.
final class LogTail: @unchecked Sendable {
    static let capacity = 2000

    private let lock = NSLock()
    private var lines: [String] = []
    private var last = 0
    private(set) var operation: (any LogOperation)?

    lazy var consumer: any DataConsumer = FBBlockDataConsumer.asynchronousLineConsumer { [weak self] line in
        self?.append(line)
    }

    func attach(_ operation: any LogOperation) {
        self.operation = operation
    }

    private func append(_ line: String) {
        if line.hasPrefix("Filtering the log data") || line.hasPrefix("Timestamp ") { return }
        lock.lock()
        defer { lock.unlock() }
        lines.append(line)
        last += 1
        if lines.count > Self.capacity { lines.removeFirst(lines.count - Self.capacity) }
    }

    /// The lines after `cursor`, at most `limit` of them, and how many older ones were already gone.
    func read(after cursor: Int, limit: Int) -> [String: Any] {
        lock.lock()
        defer { lock.unlock() }
        let first = last - lines.count + 1
        let start = max(cursor + 1, first)
        let end = min(last, start + limit - 1)
        let slice = start <= end ? Array(lines[(start - first)...(end - first)]) : []
        return ["lines": slice, "cursor": start <= end ? end : max(cursor, start - 1), "dropped": max(0, first - cursor - 1), "more": end < last]
    }
}
