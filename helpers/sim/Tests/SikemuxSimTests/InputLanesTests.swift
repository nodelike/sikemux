import XCTest

@testable import SikemuxSim

final class InputLanesTests: XCTestCase {
    private actor Journal {
        var entries: [String] = []
        func note(_ entry: String) { entries.append(entry) }
    }

    func testInputForOneDeviceRunsInArrivalOrderWithoutOverlapping() async throws {
        let lanes = InputLanes()
        let journal = Journal()
        let typing = lanes.run(on: "phone") {
            await journal.note("text starts")
            try await Task.sleep(nanoseconds: 50_000_000)
            await journal.note("text ends")
        }
        let touch = lanes.run(on: "phone") { await journal.note("touch") }
        let elsewhere = lanes.run(on: "tablet") { await journal.note("other device") }
        _ = try await (typing.value, touch.value, elsewhere.value)
        let entries = await journal.entries
        XCTAssertEqual(entries.filter { $0 != "other device" }, ["text starts", "text ends", "touch"])
        XCTAssertLessThan(entries.firstIndex(of: "other device")!, entries.firstIndex(of: "text ends")!)
    }

    func testAFailureDoesNotStopTheLane() async throws {
        let lanes = InputLanes()
        let failing = lanes.run(on: "phone") { throw Failure(reason: "x", message: "no") }
        let next = lanes.run(on: "phone") { 7 }
        await XCTAssertThrowsErrorAsync(try await failing.value)
        let value = try await next.value
        XCTAssertEqual(value, 7)
    }

    private func XCTAssertThrowsErrorAsync(_ body: @autoclosure () async throws -> Void) async {
        do {
            try await body()
            XCTFail("expected an error")
        } catch {}
    }
}
