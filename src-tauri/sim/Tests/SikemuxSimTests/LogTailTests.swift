import XCTest

@testable import SikemuxSim

final class LogTailTests: XCTestCase {
    private func line(_ process: String, _ message: String) -> String {
        "2026-10-06 08:26:18.980 I  \(process)[59951:25b52] (CoreHaptics) \(message)"
    }

    func testNamesTheProcessBeforeItsBracket() {
        XCTAssertEqual(LogTail.process(of: line("SimFixture", "tapped")), "SimFixture")
        XCTAssertEqual(
            LogTail.process(of: "2026-10-06 08:26:18.980 Df SimFixture[1:2] [com.nodelike.fixture:fixture] tapped"), "SimFixture")
        XCTAssertNil(LogTail.process(of: "Filtering the log data using \"process == x\""))
    }

    func testKeepsAProcessesLinesWhileOthersFillTheDevicesLog() {
        let tail = LogTail()
        tail.append(line("SimFixture", "tapped add one, count 1"))
        for index in 0..<(LogTail.capacity + 10) { tail.append(line("SpringBoard", "noise \(index)")) }

        let device = tail.read(after: 0, limit: 5)
        XCTAssertEqual(device["dropped"] as? Int, 11)
        XCTAssertFalse((device["lines"] as? [String] ?? []).contains { $0.contains("tapped") })

        let app = tail.read(process: "SimFixture", after: 0, limit: 5)
        XCTAssertEqual(app["lines"] as? [String], [line("SimFixture", "tapped add one, count 1")])
        XCTAssertEqual(app["cursor"] as? Int, 1)
        XCTAssertEqual(app["more"] as? Bool, false)
    }

    func testReadsOnFromAProcesssOwnCursor() {
        let tail = LogTail()
        for index in 1...3 {
            tail.append(line("SimFixture", "step \(index)"))
            tail.append(line("SpringBoard", "noise \(index)"))
        }
        let first = tail.read(process: "SimFixture", after: 0, limit: 2)
        XCTAssertEqual(first["cursor"] as? Int, 2)
        XCTAssertEqual(first["more"] as? Bool, true)
        let rest = tail.read(process: "SimFixture", after: 2, limit: 2)
        XCTAssertEqual(rest["lines"] as? [String], [line("SimFixture", "step 3")])
        XCTAssertEqual(tail.read(process: "Notes", after: 0, limit: 2)["lines"] as? [String], [])
    }

    func testNamesAProcessWithSpacesInItsName() {
        XCTAssertEqual(LogTail.process(of: line("My App", "launched")), "My App")
        XCTAssertEqual(LogTail.process(of: "2026-10-06 08:26:18.980 Df My App[123:4] [sub:cat] [x]"), "My App")
        XCTAssertNil(LogTail.process(of: "a continued message with a bracket[1:2]"))
    }

    func testKeepsTheNewestLinesInOrderOnceFull() {
        let tail = LogTail()
        for index in 1...(LogTail.capacity * 2 + 3) { tail.append(line("SpringBoard", "\(index)")) }
        let read = tail.read(after: 0, limit: 3)
        XCTAssertEqual(read["lines"] as? [String], (LogTail.capacity + 4...LogTail.capacity + 6).map { line("SpringBoard", "\($0)") })
        XCTAssertEqual(read["dropped"] as? Int, LogTail.capacity + 3)
        let newest = tail.read(after: LogTail.capacity * 2 + 2, limit: 5)
        XCTAssertEqual(newest["lines"] as? [String], [line("SpringBoard", "\(LogTail.capacity * 2 + 3)")])
        XCTAssertEqual(newest["more"] as? Bool, false)
    }

    func testForgetsTheLeastRecentlyUsedProcessBeyondTheLimit() {
        let tail = LogTail()
        tail.append(line("Kept", "first"))
        tail.append(line("Dropped", "first"))
        for index in 0..<(LogTail.processLimit - 2) { tail.append(line("Other\(index)", "noise")) }
        _ = tail.read(process: "Kept", after: 0, limit: 1)
        tail.append(line("OneTooMany", "noise"))
        XCTAssertEqual((tail.read(process: "Kept", after: 0, limit: 5)["lines"] as? [String])?.count, 1)
        XCTAssertEqual(tail.read(process: "Dropped", after: 0, limit: 5)["lines"] as? [String], [])
    }

    func testACursorFromAnotherTailOrPastTheNewestLineStartsAgain() {
        let tail = LogTail()
        for index in 1...3 { tail.append(line("SimFixture", "step \(index)")) }
        let ahead = tail.read(after: 40, limit: 2)
        XCTAssertEqual(ahead["reset"] as? Bool, true)
        XCTAssertEqual(ahead["lines"] as? [String], [line("SimFixture", "step 1"), line("SimFixture", "step 2")])
        XCTAssertEqual(ahead["generation"] as? Int, tail.generation)

        let elsewhere = tail.read(after: 2, limit: 5, generation: tail.generation + 1)
        XCTAssertEqual(elsewhere["reset"] as? Bool, true)
        XCTAssertEqual((elsewhere["lines"] as? [String])?.count, 3)

        let same = tail.read(after: 2, limit: 5, generation: tail.generation)
        XCTAssertEqual(same["reset"] as? Bool, false)
        XCTAssertEqual(same["lines"] as? [String], [line("SimFixture", "step 3")])
        XCTAssertEqual(tail.read(after: 3, limit: 5)["reset"] as? Bool, false)
    }
}
