import XCTest

@testable import SikemuxSim

final class RequestTests: XCTestCase {
    func testReadsARequest() throws {
        let request = try Request.read(#"{"id":4,"type":"tap","x":1,"y":2}"#).get()
        XCTAssertEqual(request.id, 4)
        XCTAssertEqual(request.type, "tap")
    }

    func testARequestThatCannotBeReadIsAnsweredByItsId() {
        guard case let .failure(unreadable) = Request.read(#"{"id":9,"type":"tap","x":"left"}"#) else { return XCTFail() }
        XCTAssertEqual(unreadable.id, 9)
        XCTAssertTrue(unreadable.message.contains("x"), unreadable.message)
    }

    func testALineWithNoIdIsAnsweredByNone() {
        guard case let .failure(unreadable) = Request.read("not json") else { return XCTFail() }
        XCTAssertNil(unreadable.id)
    }

    func testErrorsAreDescribedInWords() {
        struct Described: LocalizedError { var errorDescription: String? { "in words" } }
        XCTAssertEqual(Failure.describe(Described()), "in words")
        XCTAssertEqual(Failure.describe(Failure(reason: "x", message: "the message")), "the message")
    }
}
