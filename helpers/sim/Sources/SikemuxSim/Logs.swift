import FBControlCore
import Foundation

/// The latest lines of one device's log, numbered from 1 so a reader can ask for what came after the last line it saw.
/// Each process's lines are also kept apart, so a chatty process cannot push another's out before it is read.
final class LogTail: @unchecked Sendable {
    static let capacity = 2000
    static let processLimit = 64

    /// Differs between tails, so a cursor numbered by another one, such as a helper that has since
    /// restarted, is recognised and read from the start.
    let generation = Int.random(in: 1...Int(Int32.max))
    private let lock = NSLock()
    private var all = Lines()
    private var byProcess: [String: (lines: Lines, used: Int)] = [:]
    private var uses = 0
    private var following: Task<Void, Never>?

    lazy var consumer: any DataConsumer = FBBlockDataConsumer.asynchronousLineConsumer { [weak self] line in
        self?.append(line)
    }

    func attach(_ operation: any LogOperation) {
        following = Task { _ = try? await operation.waitUntilCompleted() }
    }

    func stop() {
        following?.cancel()
    }

    func append(_ line: String) {
        if line.hasPrefix("Filtering the log data") || line.hasPrefix("Timestamp ") { return }
        lock.lock()
        defer { lock.unlock() }
        all.append(line)
        guard let process = Self.process(of: line) else { return }
        var lines = byProcess.removeValue(forKey: process)?.lines ?? Lines()
        lines.append(line)
        uses += 1
        byProcess[process] = (lines, uses)
        if byProcess.count > Self.processLimit, let stalest = byProcess.min(by: { $0.value.used < $1.value.used })?.key {
            byProcess[stalest] = nil
        }
    }

    /// The lines after `cursor`, at most `limit` of them, and how many older ones were already gone. Named a
    /// process, the lines and the cursor are that process's own. A cursor from another tail, or past the
    /// newest line, is read from the start and the answer says `reset`.
    func read(process: String? = nil, after cursor: Int, limit: Int, generation: Int? = nil) -> [String: Any] {
        lock.lock()
        defer { lock.unlock() }
        let lines: Lines
        if let process {
            lines = byProcess[process]?.lines ?? Lines()
            if byProcess[process] != nil {
                uses += 1
                byProcess[process]?.used = uses
            }
        } else {
            lines = all
        }
        let reset = (generation != nil && generation != self.generation) || cursor > lines.last
        var answer = lines.read(after: reset ? 0 : cursor, limit: limit)
        answer["reset"] = reset
        answer["generation"] = self.generation
        return answer
    }

    private static let compact = try! NSRegularExpression(
        pattern: #"^\d{4}-\d\d-\d\d \d\d:\d\d:\d\d\.\d+ +\S+ +(.+?)\[\d+:[0-9a-fA-F]+\]"#)

    /// A compact log line names its process, which may hold spaces, before the bracket holding its ids:
    /// `2026-10-06 14:00:00.123 Df My App[1234:5678] message`.
    static func process(of line: String) -> String? {
        let range = NSRange(line.startIndex..., in: line)
        guard let match = compact.firstMatch(in: line, range: range), let name = Range(match.range(at: 1), in: line) else { return nil }
        return String(line[name])
    }
}

/// Numbered lines, the oldest overwritten once `LogTail.capacity` are kept.
struct Lines {
    private var ring: [String] = []
    private(set) var last = 0

    mutating func append(_ line: String) {
        if ring.count < LogTail.capacity {
            ring.append(line)
        } else {
            ring[last % LogTail.capacity] = line
        }
        last += 1
    }

    func read(after cursor: Int, limit: Int) -> [String: Any] {
        let first = last - ring.count + 1
        let start = max(cursor + 1, first)
        let end = min(last, start + limit - 1)
        let slice = start <= end ? (start...end).map { ring[($0 - 1) % LogTail.capacity] } : []
        return ["lines": slice, "cursor": start <= end ? end : max(cursor, start - 1), "dropped": max(0, first - cursor - 1), "more": end < last]
    }
}
